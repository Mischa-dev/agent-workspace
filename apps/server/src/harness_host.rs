//! The server side of `provider::host::HarnessHost` (W12): adapters ask it
//! for an account's stored key, a registered harness's secrets and a run's
//! repository, and never reach the database or secret store themselves.
//!
//! Keys are the account owner's own secrets (S1, owner = the person), kept
//! system-managed so they don't show in the person's secret list; a
//! registered command harness gets only the owner's secrets it names.

use std::{path::Path, sync::Arc};

use secrecy::ExposeSecret;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    config::AppConfig,
    provider::{
        HarnessError,
        adapter::HarnessFuture,
        host::{HarnessHost, RepositoryAccess, account_secret_name},
    },
    secrets::{self, NewSecret, SecretOwner, SecretValue},
};

pub struct ServerHost {
    pool: PgPool,
    config: Arc<AppConfig>,
}

impl ServerHost {
    pub fn new(pool: PgPool, config: Arc<AppConfig>) -> Self {
        Self { pool, config }
    }

    async fn owner(&self, account_id: Uuid) -> Result<SecretOwner, HarnessError> {
        let (organization_id, actor_id) = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT organization_id, owner_actor_id FROM provider_accounts WHERE id = $1",
        )
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(failed)?
        .ok_or(HarnessError::MissingResult("the account"))?;
        Ok(SecretOwner::Actor {
            organization_id,
            actor_id,
        })
    }

    fn keys(&self) -> Result<Arc<dyn secrets::KeyEncryption>, HarnessError> {
        secrets::keyring(&self.config).map_err(|_| HarnessError::Unsupported("the secret store"))
    }
}

fn failed(error: impl std::error::Error + Send + Sync + 'static) -> HarnessError {
    HarnessError::Transport(Box::new(error))
}

fn app_failed(error: crate::error::AppError) -> HarnessError {
    tracing::warn!(
        error_category = error.log_category(),
        "harness host request failed"
    );
    HarnessError::MissingResult("a stored secret")
}

impl HarnessHost for ServerHost {
    fn account_secret(&self, account_id: Uuid) -> HarnessFuture<'_, Option<SecretValue>> {
        Box::pin(async move {
            let owner = self.owner(account_id).await?;
            let name = account_secret_name(account_id);
            if secrets::find(&self.pool, owner, &name)
                .await
                .map_err(app_failed)?
                .is_none()
            {
                return Ok(None);
            }
            let keys = self.keys()?;
            secrets::reveal_named(
                &self.pool,
                keys.as_ref(),
                owner,
                &name,
                "harness.account_key",
            )
            .await
            .map_err(app_failed)
        })
    }

    fn store_account_secret<'a>(
        &'a self,
        account_id: Uuid,
        value: &'a SecretValue,
    ) -> HarnessFuture<'a, ()> {
        Box::pin(async move {
            let owner = self.owner(account_id).await?;
            let keys = self.keys()?;
            let mut connection = self.pool.acquire().await.map_err(failed)?;
            secrets::put(
                &mut connection,
                keys.as_ref(),
                NewSecret {
                    owner,
                    name: &account_secret_name(account_id),
                    description: Some("The key a harness account signs in with."),
                    value,
                    actor_id: None,
                    system_managed: true,
                },
            )
            .await
            .map_err(app_failed)?;
            Ok(())
        })
    }

    fn delete_account_secret(&self, account_id: Uuid) -> HarnessFuture<'_, ()> {
        Box::pin(async move {
            let owner = self.owner(account_id).await?;
            if let Some(id) = secrets::find(&self.pool, owner, &account_secret_name(account_id))
                .await
                .map_err(app_failed)?
            {
                secrets::delete(&self.pool, id).await.map_err(app_failed)?;
            }
            Ok(())
        })
    }

    fn harness_secrets<'a>(
        &'a self,
        account_id: Uuid,
        names: &'a [String],
    ) -> HarnessFuture<'a, Vec<(String, SecretValue)>> {
        Box::pin(async move {
            if names.is_empty() {
                return Ok(Vec::new());
            }
            let owner = self.owner(account_id).await?;
            let keys = self.keys()?;
            let mut found = Vec::new();
            for name in names {
                // A key the server keeps for itself is never handed out by
                // name, even if a registration names it.
                if name.contains(':') {
                    continue;
                }
                if let Some(value) = secrets::reveal_named(
                    &self.pool,
                    keys.as_ref(),
                    owner,
                    name,
                    "harness.command_secret",
                )
                .await
                .map_err(app_failed)?
                {
                    found.push((name.clone(), value));
                }
            }
            Ok(found)
        })
    }

    fn repository_access<'a>(&'a self, worktree: &'a Path) -> HarnessFuture<'a, RepositoryAccess> {
        Box::pin(async move {
            let path = worktree.to_string_lossy().into_owned();
            let (project_id, repository_id, owner, name) =
                sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
                    r#"
                    SELECT repository.project_id, repository.id, repository.owner, repository.name
                    FROM git_worktrees worktree
                    JOIN repository_workspaces workspace
                      ON workspace.id = worktree.repository_workspace_id
                    JOIN repositories repository ON repository.id = workspace.repository_id
                    WHERE worktree.path = $1
                    "#,
                )
                .bind(&path)
                .fetch_optional(&self.pool)
                .await
                .map_err(failed)?
                .ok_or(HarnessError::MissingResult("the run's repository"))?;
            let token = crate::github::repository_git_token_with(
                &self.pool,
                &self.config,
                project_id,
                repository_id,
            )
            .await
            .map_err(app_failed)?;
            Ok(RepositoryAccess {
                url: format!("https://github.com/{owner}/{name}"),
                token: SecretValue::new(token.expose_secret()),
            })
        })
    }
}

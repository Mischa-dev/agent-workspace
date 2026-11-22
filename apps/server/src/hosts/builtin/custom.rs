//! The harnesses a workspace registers itself (`crate::harnesses`), as the
//! built-in host runs them: each registration becomes an adapter in the
//! host's registry, reloaded from the database after each change and every
//! half minute, so every process that carries the host agrees.

use std::{path::PathBuf, sync::Arc, time::Duration};

use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    provider::{
        acp::AcpAdapter,
        adapter::HarnessAdapter,
        command_line::{self, CommandAdapter, CommandSpec, RunnerAdapter},
        registry::CustomAdapter,
    },
};

const RELOAD_EVERY: Duration = Duration::from_secs(30);

/// What the host needs of a registration to run it.
#[derive(FromRow)]
struct Registration {
    id: Uuid,
    organization_id: Uuid,
    kind: String,
    name: String,
    transport: String,
    manifest: Value,
    command: Option<Value>,
    secret_names: Vec<String>,
    runner_actor_id: Option<Uuid>,
    enabled: bool,
}

/// Whether a registered command can be run by the host: its program is
/// in one of the directories the operator allows, with few enough
/// arguments. The reason, for the settings page, when it can't.
pub fn validate_command(command: &[String], allowed_dirs: &[PathBuf]) -> Result<(), String> {
    command_line::validate_command(command, allowed_dirs).map(|_| ())
}

fn adapter_for(state: &AppState, row: &Registration) -> Result<CustomAdapter, String> {
    let spec = |manifest| CommandSpec {
        manifest,
        command: row
            .command
            .as_ref()
            .and_then(|command| serde_json::from_value(command.clone()).ok())
            .unwrap_or_default(),
        secret_names: row.secret_names.clone(),
        allowed_dirs: state.config.command_harness_dirs.clone(),
    };
    let manifest =
        crate::harnesses::manifest_of(&row.kind, &row.name, &row.transport, &row.manifest)?;
    let adapter: Arc<dyn HarnessAdapter> = match row.transport.as_str() {
        "runner" => Arc::new(RunnerAdapter::new(manifest)),
        "acp" => Arc::new(AcpAdapter::new(
            spec(manifest),
            state.config.codex_host_concurrency_limit,
            state.supervisors.host_cell(),
        )),
        _ => Arc::new(CommandAdapter::new(
            spec(manifest),
            state.config.codex_host_concurrency_limit,
            state.supervisors.host_cell(),
        )),
    };
    Ok(CustomAdapter {
        organization_id: row.organization_id,
        enabled: row.enabled,
        runner_actor_id: row.runner_actor_id,
        adapter,
    })
}

/// Loads every registered harness into the host's registry.
pub async fn reload(state: &AppState) -> Result<(), AppError> {
    // Every workspace's registered harnesses.
    crate::tenancy::system("harnesses.reload", Box::pin(reload_here(state))).await
}

async fn reload_here(state: &AppState) -> Result<(), AppError> {
    let rows = sqlx::query_as::<_, Registration>(
        "SELECT id, organization_id, kind, name, transport, manifest, command, secret_names, \
         runner_actor_id, enabled FROM custom_harnesses ORDER BY created_at",
    )
    .fetch_all(crate::db::scoped(&state.pool))
    .await?;
    let adapters = rows
        .iter()
        .filter_map(|row| match adapter_for(state, row) {
            Ok(adapter) => Some(adapter),
            Err(_) => {
                tracing::warn!(harness_id = %row.id, "a registered harness's manifest no longer loads");
                None
            }
        })
        .collect();
    state.supervisors.replace_custom(adapters);
    Ok(())
}

/// Keeps the registry in step with the database for the life of the server.
pub fn spawn_reloader(state: AppState) {
    tokio::spawn(async move {
        loop {
            // Every workspace's registered harnesses.
            if let Err(error) = crate::tenancy::system("harnesses.reload", reload(&state)).await {
                tracing::warn!(
                    error_category = error.log_category(),
                    "registered harnesses not reloaded"
                );
            }
            tokio::select! {
                () = tokio::time::sleep(RELOAD_EVERY) => {}
                () = state.supervisors.shutdown_requested() => return,
            }
        }
    });
}

//! Registering your own harness (X2): a command on this server (H7's
//! protocol), an agent on this server that speaks the Agent Client
//! Protocol (A1.21), or a connected runner (an agent with a runner, X1),
//! with a manifest. A registered harness gets its own adapter kind (`x_<name>`),
//! so accounts, the model catalog, run settings, agent profiles and the
//! Launch dialog treat it like Codex. Only its own workspace sees it.
//!
//! Workspace admins register and change harnesses; every member can see
//! them. The registry is reloaded from the database after each change and
//! every half minute, so every server process agrees.

use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, patch},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    harness::{
        manifest::{AdapterManifest, RunLocation},
        registered,
    },
    provider::{
        command_line::{self, CommandAdapter, CommandSpec, RunnerAdapter},
        registry::CustomAdapter,
    },
};

const MAX_HARNESSES: i64 = 50;
const RELOAD_EVERY: Duration = Duration::from_secs(30);

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/organizations/{organization_id}/harnesses",
            get(list).post(create),
        )
        .route(
            "/api/v1/organizations/{organization_id}/harnesses/{harness_id}",
            patch(update).delete(remove),
        )
}

#[derive(Clone, Debug, FromRow)]
struct HarnessRow {
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
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, organization_id, kind, name, transport, manifest, command, \
    secret_names, runner_actor_id, enabled, version, created_at, updated_at";

fn command_of(row: &HarnessRow) -> Vec<String> {
    row.command
        .as_ref()
        .and_then(|command| serde_json::from_value(command.clone()).ok())
        .unwrap_or_default()
}

/// The manifest a registration gives its harness: what the host runs it by
/// and what the server offers and checks launches against.
pub fn manifest_of(
    kind: &str,
    name: &str,
    transport: &str,
    manifest: &Value,
) -> Result<AdapterManifest, String> {
    match transport {
        "acp" => registered::acp_manifest(kind, name, manifest),
        "runner" => registered::command_line_manifest(kind, name, manifest, RunLocation::Cloud),
        _ => registered::command_line_manifest(kind, name, manifest, RunLocation::LocalHost),
    }
}

fn adapter_for(state: &AppState, row: &HarnessRow) -> Result<CustomAdapter, String> {
    let spec = |manifest| CommandSpec {
        manifest,
        command: command_of(row),
        secret_names: row.secret_names.clone(),
        allowed_dirs: state.config.command_harness_dirs.clone(),
    };
    let manifest = manifest_of(&row.kind, &row.name, &row.transport, &row.manifest)?;
    let adapter: Arc<dyn crate::provider::adapter::HarnessAdapter> = match row.transport.as_str() {
        "runner" => Arc::new(RunnerAdapter::new(manifest)),
        "acp" => Arc::new(crate::provider::acp::AcpAdapter::new(
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

/// Loads every registered harness into the registry.
pub async fn reload(state: &AppState) -> Result<(), AppError> {
    // Every workspace's registered harnesses.
    crate::tenancy::system("harnesses.reload", Box::pin(reload_here(state))).await
}

async fn reload_here(state: &AppState) -> Result<(), AppError> {
    let rows = sqlx::query_as::<_, HarnessRow>(&format!(
        "SELECT {COLUMNS} FROM custom_harnesses ORDER BY created_at"
    ))
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessView {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
    pub transport: String,
    pub manifest: Value,
    /// What the rest of the server reads: the checked manifest, or `None`
    /// when it no longer loads.
    pub resolved: Option<AdapterManifest>,
    pub command: Option<Vec<String>>,
    pub secret_names: Vec<String>,
    pub runner_actor_id: Option<Uuid>,
    pub runner_name: Option<String>,
    pub enabled: bool,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessList {
    pub harnesses: Vec<HarnessView>,
    pub can_manage: bool,
    /// Whether this server lets command-line harnesses run at all.
    pub commands_allowed: bool,
    /// Agents that can be picked as a connected runner.
    pub runners: Vec<RunnerChoice>,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RunnerChoice {
    pub actor_id: Uuid,
    pub display_name: String,
}

async fn view(state: &AppState, row: HarnessRow) -> Result<HarnessView, AppError> {
    let resolved = adapter_for(state, &row)
        .ok()
        .map(|custom| custom.adapter.manifest().clone());
    let runner_name = match row.runner_actor_id {
        Some(actor_id) => {
            sqlx::query_scalar::<_, String>(
                r#"
            SELECT membership.display_name FROM project_memberships membership
            JOIN projects project ON project.id = membership.project_id
            WHERE membership.actor_id = $1 AND project.organization_id = $2
            ORDER BY membership.created_at LIMIT 1
            "#,
            )
            .bind(actor_id)
            .bind(row.organization_id)
            .fetch_optional(crate::db::scoped(&state.pool))
            .await?
        }
        None => None,
    };
    let command = runs_program(&row.transport).then(|| command_of(&row));
    Ok(HarnessView {
        id: row.id,
        kind: row.kind,
        name: row.name,
        transport: row.transport,
        manifest: row.manifest,
        resolved,
        command,
        secret_names: row.secret_names,
        runner_actor_id: row.runner_actor_id,
        runner_name,
        enabled: row.enabled,
        version: row.version,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

/// The harnesses a workspace registered, for any of its people.
pub async fn list_for(
    state: &AppState,
    principal: &AuthenticatedActor,
    organization_id: Uuid,
) -> Result<HarnessList, AppError> {
    crate::settings::authorize_personal(&state.pool, principal, organization_id).await?;
    let can_manage =
        crate::settings::authorize_workspace_manage(&state.pool, principal, organization_id)
            .await
            .is_ok();
    let rows = sqlx::query_as::<_, HarnessRow>(&format!(
        "SELECT {COLUMNS} FROM custom_harnesses WHERE organization_id = $1 ORDER BY name"
    ))
    .bind(organization_id)
    .fetch_all(crate::db::scoped(&state.pool))
    .await?;
    let mut harnesses = Vec::with_capacity(rows.len());
    for row in rows {
        harnesses.push(view(state, row).await?);
    }
    let runners = sqlx::query_as::<_, RunnerChoice>(
        r#"
        SELECT DISTINCT ON (actor.id) actor.id AS actor_id, membership.display_name
        FROM actors actor
        JOIN project_memberships membership ON membership.actor_id = actor.id AND membership.enabled
        JOIN projects project ON project.id = membership.project_id
        WHERE project.organization_id = $1
          AND actor.kind = 'external_agent' AND actor.status = 'active' -- // kind-check: mechanical (a runner signs in through an agent connection)
        ORDER BY actor.id, membership.created_at
        "#,
    )
    .bind(organization_id)
    .fetch_all(crate::db::scoped(&state.pool))
    .await?;
    Ok(HarnessList {
        harnesses,
        can_manage,
        commands_allowed: !state.config.command_harness_dirs.is_empty(),
        runners,
    })
}

async fn list(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<HarnessList>, AppError> {
    Ok(Json(list_for(&state, &principal, organization_id).await?))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRequest {
    name: String,
    transport: String,
    manifest: Value,
    #[serde(default)]
    command: Option<Vec<String>>,
    #[serde(default)]
    secret_names: Vec<String>,
    #[serde(default)]
    runner_actor_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRequest {
    version: i64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    manifest: Option<Value>,
    #[serde(default)]
    command: Option<Vec<String>>,
    #[serde(default)]
    secret_names: Option<Vec<String>>,
    #[serde(default)]
    runner_actor_id: Option<Uuid>,
    #[serde(default)]
    enabled: Option<bool>,
}

fn valid_name(name: &str) -> Result<String, AppError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(AppError::Validation("name"));
    }
    Ok(name.to_owned())
}

fn kind_for(name: &str) -> String {
    let mut slug = String::new();
    for character in name.to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.ends_with('_') && !slug.is_empty() {
            slug.push('_');
        }
    }
    let slug = slug.trim_end_matches('_');
    let slug: String = if slug.is_empty() {
        "harness".to_owned()
    } else {
        slug.chars().take(32).collect()
    };
    format!("x_{}", slug.trim_end_matches('_'))
}

fn valid_secret_names(names: &[String]) -> Result<(), AppError> {
    if names.len() > 16 {
        return Err(AppError::Validation("secretNames"));
    }
    for name in names {
        crate::secrets::validate_name(name).map_err(|_| AppError::Validation("secretNames"))?;
        // Names with a colon are the server's own (account keys, webhooks).
        if name.contains(':') {
            return Err(AppError::Validation("secretNames"));
        }
    }
    Ok(())
}

async fn valid_runner(
    state: &AppState,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<(), AppError> {
    let ok = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM actors actor
            JOIN project_memberships membership ON membership.actor_id = actor.id
            JOIN projects project ON project.id = membership.project_id
            WHERE actor.id = $1 AND project.organization_id = $2
              AND actor.kind = 'external_agent' AND actor.status = 'active' -- // kind-check: mechanical (a runner signs in through an agent connection)
        )
        "#,
    )
    .bind(actor_id)
    .bind(organization_id)
    .fetch_one(crate::db::scoped(&state.pool))
    .await?;
    if ok {
        Ok(())
    } else {
        Err(AppError::Validation("runnerActorId"))
    }
}

/// Checks a whole registration as it would be stored.
async fn check(state: &AppState, row: &HarnessRow) -> Result<(), AppError> {
    valid_secret_names(&row.secret_names)?;
    match row.transport.as_str() {
        "command" | "acp" => {
            command_line::validate_command(&command_of(row), &state.config.command_harness_dirs)
                .map_err(|_| AppError::Validation("command"))?;
        }
        "runner" => {
            let actor_id = row
                .runner_actor_id
                .ok_or(AppError::Validation("runnerActorId"))?;
            valid_runner(state, row.organization_id, actor_id).await?;
        }
        _ => return Err(AppError::Validation("transport")),
    }
    adapter_for(state, row).map_err(|_| AppError::Validation("manifest"))?;
    Ok(())
}

/// The reason a registration was refused, for the settings page.
fn explain(state: &AppState, row: &HarnessRow) -> Option<String> {
    if runs_program(&row.transport)
        && let Err(reason) =
            command_line::validate_command(&command_of(row), &state.config.command_harness_dirs)
    {
        return Some(reason);
    }
    manifest_of(&row.kind, &row.name, &row.transport, &row.manifest).err()
}

/// Whether a registration runs a program on this host: a command-line
/// harness or an agent spoken to over the Agent Client Protocol.
fn runs_program(transport: &str) -> bool {
    matches!(transport, "command" | "acp")
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Refusal {
    error: RefusalBody,
}

/// The same shape as every other API error, with the reason in words.
#[derive(Debug, Serialize)]
struct RefusalBody {
    code: &'static str,
    message: String,
}

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateRequest>,
) -> Result<axum::response::Response, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    crate::settings::authorize_workspace_manage(&state.pool, &principal, organization_id).await?;
    let name = valid_name(&request.name)?;
    let now = Utc::now();
    let mut row = HarnessRow {
        id: Uuid::now_v7(),
        organization_id,
        kind: kind_for(&name),
        name,
        transport: request.transport.clone(),
        manifest: request.manifest.clone(),
        command: request
            .command
            .clone()
            .filter(|_| runs_program(&request.transport))
            .map(|command| json!(command)),
        secret_names: request.secret_names.clone(),
        runner_actor_id: request
            .runner_actor_id
            .filter(|_| request.transport == "runner"),
        enabled: true,
        version: 1,
        created_at: now,
        updated_at: now,
    };
    if let Some(message) = explain(&state, &row) {
        return Ok(refused(message));
    }
    check(&state, &row).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM custom_harnesses WHERE organization_id = $1",
    )
    .bind(organization_id)
    .fetch_one(&mut *transaction)
    .await?;
    if count >= MAX_HARNESSES {
        return Err(AppError::Conflict("harness", "too many harnesses"));
    }
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM custom_harnesses WHERE organization_id = $1 AND name = $2)",
    )
    .bind(organization_id)
    .bind(&row.name)
    .fetch_one(&mut *transaction)
    .await?
    {
        return Err(AppError::Conflict("harness", "name in use"));
    }
    // Kinds are unique across the server; another workspace may have
    // taken this one, and its row is hidden from this one, so the insert
    // itself finds out (and the next suffix is tried).
    let base = row.kind.clone();
    let mut inserted = false;
    for attempt in 0..20 {
        if attempt > 0 {
            row.kind = format!("{}_{}", base, attempt + 1);
        }
        if crate::harness_catalog::is_builtin(&mut transaction, &row.kind).await? {
            continue;
        }
        inserted = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO custom_harnesses (
                id, organization_id, kind, name, transport, manifest, command,
                secret_names, runner_actor_id, created_by_actor_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (kind) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(row.id)
        .bind(organization_id)
        .bind(&row.kind)
        .bind(&row.name)
        .bind(&row.transport)
        .bind(&row.manifest)
        .bind(&row.command)
        .bind(&row.secret_names)
        .bind(row.runner_actor_id)
        .bind(principal.actor_id)
        .fetch_optional(&mut *transaction)
        .await?
        .is_some();
        if inserted {
            break;
        }
    }
    if !inserted {
        return Err(AppError::Conflict("harness", "no free kind for this name"));
    }
    crate::settings::audit(
        &mut transaction,
        &headers,
        &principal,
        None,
        "harness.registered",
        "custom_harness",
        Some(row.id),
        json!({ "kind": row.kind, "transport": row.transport }),
    )
    .await?;
    transaction.commit().await?;
    reload(&state).await?;
    let stored = load(&state, organization_id, row.id).await?;
    Ok((StatusCode::CREATED, Json(view(&state, stored).await?)).into_response())
}

fn refused(message: String) -> axum::response::Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(Refusal {
            error: RefusalBody {
                code: "invalid_harness",
                message,
            },
        }),
    )
        .into_response()
}

async fn load(state: &AppState, organization_id: Uuid, id: Uuid) -> Result<HarnessRow, AppError> {
    sqlx::query_as::<_, HarnessRow>(&format!(
        "SELECT {COLUMNS} FROM custom_harnesses WHERE organization_id = $1 AND id = $2"
    ))
    .bind(organization_id)
    .bind(id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?
    .ok_or(AppError::NotFound("harness"))
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, harness_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateRequest>,
) -> Result<axum::response::Response, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    crate::settings::authorize_workspace_manage(&state.pool, &principal, organization_id).await?;
    let mut row = load(&state, organization_id, harness_id).await?;
    if row.version != request.version {
        return Err(AppError::Conflict("harness", "changed since you loaded it"));
    }
    if let Some(name) = &request.name {
        row.name = valid_name(name)?;
    }
    if let Some(manifest) = request.manifest {
        row.manifest = manifest;
    }
    if let Some(command) = request.command {
        if !runs_program(&row.transport) {
            return Err(AppError::Validation("command"));
        }
        row.command = Some(json!(command));
    }
    if let Some(names) = request.secret_names {
        row.secret_names = names;
    }
    if let Some(actor_id) = request.runner_actor_id {
        if row.transport != "runner" {
            return Err(AppError::Validation("runnerActorId"));
        }
        row.runner_actor_id = Some(actor_id);
    }
    if let Some(enabled) = request.enabled {
        row.enabled = enabled;
    }
    if let Some(message) = explain(&state, &row) {
        return Ok(refused(message));
    }
    check(&state, &row).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let updated = sqlx::query(
        r#"
        UPDATE custom_harnesses
        SET name = $3, manifest = $4, command = $5, secret_names = $6,
            runner_actor_id = $7, enabled = $8, version = version + 1, updated_at = now()
        WHERE organization_id = $1 AND id = $2 AND version = $9
        "#,
    )
    .bind(organization_id)
    .bind(harness_id)
    .bind(&row.name)
    .bind(&row.manifest)
    .bind(&row.command)
    .bind(&row.secret_names)
    .bind(row.runner_actor_id)
    .bind(row.enabled)
    .bind(request.version)
    .execute(&mut *transaction)
    .await
    .map_err(|error| match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            AppError::Conflict("harness", "name in use")
        }
        _ => AppError::from(error),
    })?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict("harness", "changed since you loaded it"));
    }
    crate::settings::audit(
        &mut transaction,
        &headers,
        &principal,
        None,
        "harness.updated",
        "custom_harness",
        Some(harness_id),
        json!({ "kind": row.kind, "enabled": row.enabled }),
    )
    .await?;
    transaction.commit().await?;
    reload(&state).await?;
    let stored = load(&state, organization_id, harness_id).await?;
    Ok(Json(view(&state, stored).await?).into_response())
}

async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, harness_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    crate::settings::authorize_workspace_manage(&state.pool, &principal, organization_id).await?;
    let row = load(&state, organization_id, harness_id).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    // Accounts and runs of the kind keep their history; the harness can
    // only go once nothing uses it.
    let in_use = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM provider_accounts WHERE adapter_kind = $1 AND status <> 'disabled')",
    )
    .bind(&row.kind)
    .fetch_one(&mut *transaction)
    .await?;
    if in_use {
        return Err(AppError::Conflict(
            "harness",
            "accounts still use it; turn it off instead",
        ));
    }
    sqlx::query("DELETE FROM custom_harnesses WHERE organization_id = $1 AND id = $2")
        .bind(organization_id)
        .bind(harness_id)
        .execute(&mut *transaction)
        .await?;
    crate::settings::audit(
        &mut transaction,
        &headers,
        &principal,
        None,
        "harness.removed",
        "custom_harness",
        Some(harness_id),
        json!({ "kind": row.kind }),
    )
    .await?;
    transaction.commit().await?;
    reload(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::kind_for;

    #[test]
    fn kinds_come_from_names() {
        assert_eq!(kind_for("My Agent!"), "x_my_agent");
        assert_eq!(kind_for("***"), "x_harness");
        assert!(kind_for(&"a".repeat(100)).len() <= 34);
    }
}

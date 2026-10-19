//! Environments (C1): where a project's runs execute. Each names a harness
//! (Codex on this host, Codex Cloud, a connected runner), the provider's own
//! environment ID for a cloud, the repository and branch, and setup notes.
//! The project's default is its `run.environment` setting, so launching
//! preselects it and the settings area shows where it came from.
//!
//! People see a project's environments; owners and admins change them.
//! Checking one runs as the caller, on their own provider account.

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgExecutor};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    memory,
    policy::{Action, ActorKind},
    provider::{ProfileAccount, manifest::RunLocation},
    settings,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/environments",
            get(list_environments).post(create_environment),
        )
        .route(
            "/api/v1/projects/{project_id}/environments/{environment_id}",
            put(update_environment),
        )
        .route(
            "/api/v1/projects/{project_id}/environments/{environment_id}/archive",
            post(archive_environment),
        )
        .route(
            "/api/v1/projects/{project_id}/environments/{environment_id}/default",
            post(make_default),
        )
        .route(
            "/api/v1/projects/{project_id}/environments/{environment_id}/check",
            post(check_environment),
        )
}

#[derive(Clone, Debug, FromRow)]
pub(crate) struct EnvironmentRow {
    pub id: Uuid,
    pub name: String,
    pub harness: String,
    pub provider_environment_id: Option<String>,
    pub repository_id: Option<Uuid>,
    pub branch: Option<String>,
    pub setup_notes: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_check_ok: Option<bool>,
    pub last_check_message: Option<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const ENVIRONMENT_COLUMNS: &str = r#"
    id, name, harness, provider_environment_id,
    repository_id, branch, setup_notes, last_checked_at, last_check_ok,
    last_check_message, version, created_at, updated_at
"#;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnvironmentView {
    pub id: Uuid,
    pub name: String,
    pub harness: String,
    /// The harness's name for people, such as "Codex Cloud".
    pub harness_name: String,
    /// "local_host" or "cloud".
    pub runs_on: Option<RunLocation>,
    pub provider_environment_id: Option<String>,
    pub repository_id: Option<Uuid>,
    pub branch: Option<String>,
    pub setup_notes: String,
    pub is_default: bool,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_check_ok: Option<bool>,
    pub last_check_message: Option<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarnessChoice {
    pub kind: String,
    pub display_name: String,
    pub runs_on: RunLocation,
    /// Whether an environment for it needs the provider's environment ID.
    pub needs_provider_environment: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnvironmentList {
    pub environments: Vec<EnvironmentView>,
    pub default_environment_id: Option<Uuid>,
    /// Where the default came from: project, workspace or default (unset).
    pub default_source: settings::Source,
    pub harnesses: Vec<HarnessChoice>,
    pub can_manage: bool,
}

pub(crate) fn view(
    state: &AppState,
    row: EnvironmentRow,
    default: Option<Uuid>,
) -> EnvironmentView {
    let manifest = state.supervisors.manifest(&row.harness);
    EnvironmentView {
        is_default: default == Some(row.id),
        harness_name: manifest
            .as_ref()
            .map(|manifest| manifest.display_name.clone())
            .unwrap_or_else(|| row.harness.clone()),
        runs_on: manifest.map(|manifest| manifest.runs_on),
        id: row.id,
        name: row.name,
        harness: row.harness,
        provider_environment_id: row.provider_environment_id,
        repository_id: row.repository_id,
        branch: row.branch,
        setup_notes: row.setup_notes,
        last_checked_at: row.last_checked_at,
        last_check_ok: row.last_check_ok,
        last_check_message: row.last_check_message,
        version: row.version,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

pub(crate) fn harness_choices(state: &AppState, organization_id: Uuid) -> Vec<HarnessChoice> {
    state
        .supervisors
        .manifests_in(organization_id)
        .into_iter()
        .map(|manifest| HarnessChoice {
            kind: manifest.kind.clone(),
            display_name: manifest.display_name.clone(),
            runs_on: manifest.runs_on,
            needs_provider_environment: manifest.runs_on == RunLocation::Cloud,
        })
        .collect()
}

/// The project's workspace, once the caller may do `action` there.
pub(crate) async fn authorize_project(
    executor: impl PgExecutor<'_> + Copy,
    actor_id: Uuid,
    actor_kind: ActorKind,
    project_id: Uuid,
    action: Action,
) -> Result<Uuid, AppError> {
    memory::project_access(executor, actor_id, actor_kind, project_id, action).await?;
    sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_optional(executor)
        .await?
        .ok_or(AppError::NotFound("project"))
}

pub(crate) fn principal_kind(principal: &AuthenticatedActor) -> Result<ActorKind, AppError> {
    match principal.actor_kind.as_str() {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("invalid actor kind")),
    }
}

/// The project's default environment ID (its `run.environment` setting) as
/// it applies to `actor_id`, and where it came from.
pub(crate) async fn default_environment(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<(Option<Uuid>, settings::Source), AppError> {
    let mut connection = state.pool.acquire().await?;
    let effective = settings::resolve(
        &mut connection,
        settings::SettingsContext {
            organization_id,
            project_id: Some(project_id),
            actor_id: Some(actor_id),
        },
        &BTreeMap::new(),
    )
    .await?;
    let resolved = effective.get("run.environment");
    Ok((
        effective
            .string("run.environment")
            .and_then(|id| Uuid::parse_str(&id).ok()),
        resolved.map_or(settings::Source::Default, |resolved| resolved.source),
    ))
}

/// A project's active environments.
pub(crate) async fn active_environments(
    executor: impl PgExecutor<'_>,
    project_id: Uuid,
) -> Result<Vec<EnvironmentRow>, AppError> {
    Ok(sqlx::query_as::<_, EnvironmentRow>(&format!(
        "SELECT {ENVIRONMENT_COLUMNS} FROM project_environments \
         WHERE project_id = $1 AND archived_at IS NULL ORDER BY lower(name), id"
    ))
    .bind(project_id)
    .fetch_all(executor)
    .await?)
}

/// One active environment of the project.
pub(crate) async fn active_environment(
    executor: impl PgExecutor<'_>,
    project_id: Uuid,
    environment_id: Uuid,
) -> Result<Option<EnvironmentRow>, AppError> {
    Ok(sqlx::query_as::<_, EnvironmentRow>(&format!(
        "SELECT {ENVIRONMENT_COLUMNS} FROM project_environments \
         WHERE project_id = $1 AND id = $2 AND archived_at IS NULL"
    ))
    .bind(project_id)
    .bind(environment_id)
    .fetch_optional(executor)
    .await?)
}

pub(crate) async fn list_for(
    state: &AppState,
    actor_id: Uuid,
    actor_kind: ActorKind,
    project_id: Uuid,
) -> Result<EnvironmentList, AppError> {
    let organization_id = authorize_project(
        &state.pool,
        actor_id,
        actor_kind,
        project_id,
        Action::ProjectView,
    )
    .await?;
    let can_manage = memory::project_access(
        &state.pool,
        actor_id,
        actor_kind,
        project_id,
        Action::SettingsManage,
    )
    .await
    .is_ok();
    let (default, default_source) =
        default_environment(state, organization_id, project_id, actor_id).await?;
    let rows = active_environments(&state.pool, project_id).await?;
    let default = default.filter(|id| rows.iter().any(|row| row.id == *id));
    Ok(EnvironmentList {
        environments: rows
            .into_iter()
            .map(|row| view(state, row, default))
            .collect(),
        default_environment_id: default,
        default_source,
        harnesses: harness_choices(state, organization_id),
        can_manage,
    })
}

async fn list_environments(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<EnvironmentList>, AppError> {
    Ok(Json(
        list_for(
            &state,
            principal.actor_id,
            principal_kind(&principal)?,
            project_id,
        )
        .await?,
    ))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnvironmentRequest {
    name: String,
    harness: String,
    #[serde(default)]
    provider_environment_id: Option<String>,
    #[serde(default)]
    repository_id: Option<Uuid>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    setup_notes: String,
    /// Create only: make it the project's default.
    #[serde(default)]
    make_default: bool,
    /// Update only.
    #[serde(default)]
    expected_version: Option<i64>,
}

struct CleanEnvironment {
    name: String,
    harness: String,
    provider_environment_id: Option<String>,
    repository_id: Option<Uuid>,
    branch: Option<String>,
    setup_notes: String,
}

fn optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

async fn clean(
    state: &AppState,
    project_id: Uuid,
    request: EnvironmentRequest,
) -> Result<CleanEnvironment, AppError> {
    let name = request.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(AppError::Validation("name"));
    }
    let organization_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&state.pool)
            .await?;
    let manifest = state
        .supervisors
        .manifest_in(organization_id, &request.harness)
        .ok_or(AppError::Validation("harness"))?;
    let provider_environment_id = optional_text(request.provider_environment_id);
    if provider_environment_id
        .as_deref()
        .is_some_and(|id| id.chars().count() > 200 || id.chars().any(char::is_control))
    {
        return Err(AppError::Validation("providerEnvironmentId"));
    }
    if manifest.runs_on == RunLocation::Cloud && provider_environment_id.is_none() {
        return Err(AppError::Validation("providerEnvironmentId"));
    }
    let branch = optional_text(request.branch);
    if let Some(branch) = &branch {
        crate::git_runtime::validate_registered_ref(branch)
            .map_err(|_| AppError::Validation("branch"))?;
    }
    if request.setup_notes.chars().count() > 4000 {
        return Err(AppError::Validation("setupNotes"));
    }
    if let Some(repository_id) = request.repository_id {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM repositories WHERE project_id = $1 AND id = $2 AND archived_at IS NULL)",
        )
        .bind(project_id)
        .bind(repository_id)
        .fetch_one(&state.pool)
        .await?;
        if !exists {
            return Err(AppError::NotFound("repository"));
        }
    }
    Ok(CleanEnvironment {
        name,
        harness: manifest.kind.clone(),
        provider_environment_id,
        repository_id: request.repository_id,
        branch,
        setup_notes: request.setup_notes.trim().to_owned(),
    })
}

fn name_conflict(error: sqlx::Error) -> AppError {
    match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            AppError::Conflict("environment", "an environment with that name exists")
        }
        _ => AppError::Database(error),
    }
}

async fn audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: &'static str,
    environment_id: Uuid,
    details: Value,
) -> Result<(), AppError> {
    crate::conversations::insert_audit_event_for(
        transaction,
        headers,
        principal,
        project_id,
        action,
        "project_environment",
        environment_id,
        details,
    )
    .await
}

async fn set_default(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    organization_id: Uuid,
    project_id: Uuid,
    environment_id: Uuid,
    actor_id: Uuid,
) -> Result<(), AppError> {
    settings::set(
        transaction,
        settings::Target::Project {
            organization_id,
            project_id,
        },
        "run.environment",
        json!(environment_id.to_string()),
        None,
        actor_id,
    )
    .await?;
    Ok(())
}

async fn create_environment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<EnvironmentRequest>,
) -> Result<Json<EnvironmentView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if principal.actor_kind != "human" {
        return Err(AppError::Forbidden);
    }
    let organization_id = authorize_project(
        &state.pool,
        principal.actor_id,
        ActorKind::Human,
        project_id,
        Action::SettingsManage,
    )
    .await?;
    let make_default = request.make_default;
    let clean = clean(&state, project_id, request).await?;
    let mut transaction = state.pool.begin().await?;
    let id = Uuid::now_v7();
    let row = sqlx::query_as::<_, EnvironmentRow>(&format!(
        r#"
        INSERT INTO project_environments (
            id, organization_id, project_id, name, harness, provider_environment_id,
            repository_id, branch, setup_notes, created_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING {ENVIRONMENT_COLUMNS}
        "#
    ))
    .bind(id)
    .bind(organization_id)
    .bind(project_id)
    .bind(&clean.name)
    .bind(&clean.harness)
    .bind(&clean.provider_environment_id)
    .bind(clean.repository_id)
    .bind(&clean.branch)
    .bind(&clean.setup_notes)
    .bind(principal.actor_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(name_conflict)?;
    // The first environment becomes the default unless one is set.
    let has_default = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM settings setting
            JOIN project_environments environment
              ON environment.project_id = setting.project_id
             AND environment.id::text = setting.value #>> '{}'
             AND environment.archived_at IS NULL
            WHERE setting.scope = 'project' AND setting.project_id = $1
              AND setting.key = 'run.environment'
        )
        "#,
    )
    .bind(project_id)
    .fetch_one(&mut *transaction)
    .await?;
    if make_default || !has_default {
        set_default(
            &mut transaction,
            organization_id,
            project_id,
            id,
            principal.actor_id,
        )
        .await?;
    }
    audit(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        "environment.created",
        id,
        json!({ "harness": clean.harness }),
    )
    .await?;
    transaction.commit().await?;
    let (default, _) =
        default_environment(&state, organization_id, project_id, principal.actor_id).await?;
    Ok(Json(view(&state, row, default)))
}

async fn update_environment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, environment_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<EnvironmentRequest>,
) -> Result<Json<EnvironmentView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if principal.actor_kind != "human" {
        return Err(AppError::Forbidden);
    }
    let organization_id = authorize_project(
        &state.pool,
        principal.actor_id,
        ActorKind::Human,
        project_id,
        Action::SettingsManage,
    )
    .await?;
    let expected_version = request
        .expected_version
        .ok_or(AppError::Validation("expectedVersion"))?;
    let make_default = request.make_default;
    let clean = clean(&state, project_id, request).await?;
    let mut transaction = state.pool.begin().await?;
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM project_environments WHERE project_id = $1 AND id = $2 AND archived_at IS NULL FOR UPDATE",
    )
    .bind(project_id)
    .bind(environment_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("environment"))?;
    if current != expected_version {
        return Err(AppError::VersionConflict {
            resource: "environment",
            current_version: current,
        });
    }
    let row = sqlx::query_as::<_, EnvironmentRow>(&format!(
        r#"
        UPDATE project_environments
        SET name = $3, harness = $4, provider_environment_id = $5, repository_id = $6,
            branch = $7, setup_notes = $8, version = version + 1, updated_at = now(),
            -- A changed target needs checking again.
            last_checked_at = CASE WHEN harness = $4 AND provider_environment_id IS NOT DISTINCT FROM $5
                                   THEN last_checked_at END,
            last_check_ok = CASE WHEN harness = $4 AND provider_environment_id IS NOT DISTINCT FROM $5
                                 THEN last_check_ok END,
            last_check_message = CASE WHEN harness = $4 AND provider_environment_id IS NOT DISTINCT FROM $5
                                      THEN last_check_message END
        WHERE project_id = $1 AND id = $2
        RETURNING {ENVIRONMENT_COLUMNS}
        "#
    ))
    .bind(project_id)
    .bind(environment_id)
    .bind(&clean.name)
    .bind(&clean.harness)
    .bind(&clean.provider_environment_id)
    .bind(clean.repository_id)
    .bind(&clean.branch)
    .bind(&clean.setup_notes)
    .fetch_one(&mut *transaction)
    .await
    .map_err(name_conflict)?;
    if make_default {
        set_default(
            &mut transaction,
            organization_id,
            project_id,
            environment_id,
            principal.actor_id,
        )
        .await?;
    }
    audit(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        "environment.updated",
        environment_id,
        json!({ "harness": clean.harness, "version": row.version }),
    )
    .await?;
    transaction.commit().await?;
    let (default, _) =
        default_environment(&state, organization_id, project_id, principal.actor_id).await?;
    Ok(Json(view(&state, row, default)))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmptyRequest {}

async fn archive_environment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, environment_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<EnvironmentList>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if principal.actor_kind != "human" {
        return Err(AppError::Forbidden);
    }
    let organization_id = authorize_project(
        &state.pool,
        principal.actor_id,
        ActorKind::Human,
        project_id,
        Action::SettingsManage,
    )
    .await?;
    let mut transaction = state.pool.begin().await?;
    let archived = sqlx::query(
        "UPDATE project_environments SET archived_at = now(), updated_at = now(), version = version + 1 WHERE project_id = $1 AND id = $2 AND archived_at IS NULL",
    )
    .bind(project_id)
    .bind(environment_id)
    .execute(&mut *transaction)
    .await?;
    if archived.rows_affected() != 1 {
        return Err(AppError::NotFound("environment"));
    }
    // A default that pointed here falls back to the workspace's (or none).
    let pointed_here = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM settings WHERE scope = 'project' AND project_id = $1 AND key = 'run.environment' AND value #>> '{}' = $2)",
    )
    .bind(project_id)
    .bind(environment_id.to_string())
    .fetch_one(&mut *transaction)
    .await?;
    if pointed_here {
        settings::reset(
            &mut transaction,
            settings::Target::Project {
                organization_id,
                project_id,
            },
            "run.environment",
            None,
        )
        .await?;
    }
    audit(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        "environment.archived",
        environment_id,
        json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(
        list_for(&state, principal.actor_id, ActorKind::Human, project_id).await?,
    ))
}

async fn make_default(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, environment_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<EnvironmentList>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if principal.actor_kind != "human" {
        return Err(AppError::Forbidden);
    }
    let organization_id = authorize_project(
        &state.pool,
        principal.actor_id,
        ActorKind::Human,
        project_id,
        Action::SettingsManage,
    )
    .await?;
    let mut transaction = state.pool.begin().await?;
    active_environment(&mut *transaction, project_id, environment_id)
        .await?
        .ok_or(AppError::NotFound("environment"))?;
    set_default(
        &mut transaction,
        organization_id,
        project_id,
        environment_id,
        principal.actor_id,
    )
    .await?;
    audit(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        "environment.made_default",
        environment_id,
        json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(
        list_for(&state, principal.actor_id, ActorKind::Human, project_id).await?,
    ))
}

/// The caller's own signed-in account for a harness in this workspace.
pub(crate) async fn own_account(
    executor: impl PgExecutor<'_>,
    organization_id: Uuid,
    actor_id: Uuid,
    harness: &str,
) -> Result<Option<ProfileAccount>, AppError> {
    Ok(sqlx::query_as::<_, ProfileAccount>(
        r#"
        SELECT id AS account_id, adapter_kind, home_path, max_concurrent_runs
        FROM provider_accounts
        WHERE organization_id = $1 AND owner_actor_id = $2 AND adapter_kind = $3
          AND status = 'available' AND account_state = 'authenticated'
        ORDER BY updated_at DESC, id
        LIMIT 1
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(harness)
    .fetch_optional(executor)
    .await?)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckResult {
    ok: bool,
    message: String,
    environment: EnvironmentView,
}

async fn check_environment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, environment_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<CheckResult>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if principal.actor_kind != "human" {
        return Err(AppError::Forbidden);
    }
    let organization_id = authorize_project(
        &state.pool,
        principal.actor_id,
        ActorKind::Human,
        project_id,
        Action::WorkerInvoke,
    )
    .await?;
    let environment = active_environment(&state.pool, project_id, environment_id)
        .await?
        .ok_or(AppError::NotFound("environment"))?;
    let account = own_account(
        &state.pool,
        organization_id,
        principal.actor_id,
        &environment.harness,
    )
    .await?;
    let manifest = state.supervisors.manifest(&environment.harness);
    let (ok, message) = match (manifest, account) {
        (None, _) => (
            false,
            "This harness isn't installed on the server.".to_owned(),
        ),
        (Some(manifest), None) => (
            false,
            format!(
                "Sign in to {} in Settings > Accounts for runs first; checks run as you.",
                manifest.display_name
            ),
        ),
        (Some(manifest), Some(account)) if manifest.runs_on == RunLocation::Cloud => {
            let provider_environment = environment
                .provider_environment_id
                .clone()
                .unwrap_or_default();
            if environment.harness == crate::provider::CODEX_CLOUD_KIND {
                match state
                    .supervisors
                    .codex_cloud()
                    .check_environment(&account.handle(), &provider_environment)
                    .await
                {
                    Ok(()) => (
                        true,
                        format!("Codex Cloud found {provider_environment} for your account."),
                    ),
                    Err(_) => (
                        false,
                        format!(
                            "Codex Cloud couldn't find {provider_environment} for your account. Check the ID or label in Codex Cloud's environment settings."
                        ),
                    ),
                }
            } else {
                (
                    true,
                    format!("Your {} account is signed in.", manifest.display_name),
                )
            }
        }
        (Some(manifest), Some(_)) => (
            true,
            format!(
                "Your {} account is signed in and runs on this server.",
                manifest.display_name
            ),
        ),
    };
    let row = sqlx::query_as::<_, EnvironmentRow>(&format!(
        r#"
        UPDATE project_environments
        SET last_checked_at = now(), last_check_ok = $3, last_check_message = $4
        WHERE project_id = $1 AND id = $2
        RETURNING {ENVIRONMENT_COLUMNS}
        "#
    ))
    .bind(project_id)
    .bind(environment_id)
    .bind(ok)
    .bind(message.chars().take(300).collect::<String>())
    .fetch_one(&state.pool)
    .await?;
    let (default, _) =
        default_environment(&state, organization_id, project_id, principal.actor_id).await?;
    Ok(Json(CheckResult {
        ok,
        message,
        environment: view(&state, row, default),
    }))
}

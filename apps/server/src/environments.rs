//! Environments (C1): where a project's runs execute. Each names a harness
//! (Codex on this host, Codex Cloud, a connected runner), the provider's own
//! environment ID for a cloud, the repository and branch, and setup notes.
//! The project's default is its `run.environment` setting, so launching
//! preselects it and the settings area shows where it came from.
//!
//! People see a project's environments; owners and admins change them.
//! Checking one runs as the caller, on their own provider account.

pub mod ops;

use std::collections::BTreeMap;

use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{FromRow, PgExecutor};
use uuid::Uuid;

use agent_workspace_host_protocol::account::{AccountAnswer, AccountQuestion};

use crate::harness_catalog::Harnesses;
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, AppError},
    harness::manifest::RunLocation,
    hosts::builtin::accounts::{Asked, ProfileAccount},
    memory,
    policy::{Action, ActorKind},
    settings,
};

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/projects/{project_id}/environments",
        get(list_environments),
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
    catalog: &Harnesses,
    row: EnvironmentRow,
    default: Option<Uuid>,
) -> EnvironmentView {
    let manifest = catalog.manifest(&row.harness);
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

pub(crate) fn harness_choices(catalog: &Harnesses) -> Vec<HarnessChoice> {
    catalog
        .manifests()
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
    project_id: Uuid,
    action: Action,
) -> Result<Uuid, AppError> {
    memory::project_access(executor, actor_id, project_id, action).await?;
    sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_optional(executor)
        .await?
        .ok_or(AppError::NotFound("project"))
}

pub(crate) fn principal_kind(principal: &AuthenticatedActor) -> Result<ActorKind, AppError> {
    crate::accounts::actor_kind(&principal.actor_kind)
}

/// The project's default environment ID (its `run.environment` setting) as
/// it applies to `actor_id`, and where it came from.
pub(crate) async fn default_environment(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<(Option<Uuid>, settings::Source), AppError> {
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
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
    project_id: Uuid,
) -> Result<EnvironmentList, AppError> {
    let organization_id =
        authorize_project(&state.pool, actor_id, project_id, Action::ProjectView).await?;
    let can_manage =
        memory::project_access(&state.pool, actor_id, project_id, Action::SettingsManage)
            .await
            .is_ok();
    let (default, default_source) =
        default_environment(state, organization_id, project_id, actor_id).await?;
    let rows = active_environments(&state.pool, project_id).await?;
    let default = default.filter(|id| rows.iter().any(|row| row.id == *id));
    let catalog = Harnesses::of(state, organization_id).await?;
    Ok(EnvironmentList {
        environments: rows
            .into_iter()
            .map(|row| view(&catalog, row, default))
            .collect(),
        default_environment_id: default,
        default_source,
        harnesses: harness_choices(&catalog),
        can_manage,
    })
}

async fn list_environments(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<EnvironmentList>, AppError> {
    Ok(Json(
        list_for(&state, principal.actor_id, project_id).await?,
    ))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EnvironmentRequest {
    pub name: String,
    pub harness: String,
    #[serde(default)]
    pub provider_environment_id: Option<String>,
    #[serde(default)]
    pub repository_id: Option<Uuid>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub setup_notes: String,
    /// Make it the project's default.
    #[serde(default)]
    pub make_default: bool,
    /// Update only.
    #[serde(default)]
    pub expected_version: Option<i64>,
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

/// Whether the redaction rewrites `id` (it reads like a credential, such
/// as `Basic production`): it would reach the host changed, so a new or
/// changed one isn't taken, and a check of one saved earlier says so.
fn rewritten_on_the_way(id: Option<&str>) -> bool {
    id.is_some_and(|id| crate::run_log::redact(id) != id)
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
            .fetch_one(crate::db::scoped(&state.pool))
            .await?;
    let catalog = Harnesses::of(state, organization_id).await?;
    let manifest = catalog
        .manifest(&request.harness)
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
    if branch
        .as_deref()
        .is_some_and(|branch| !agent_workspace_host_protocol::git_ref::is_safe_ref(branch))
    {
        return Err(AppError::Validation("branch"));
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
        .fetch_one(crate::db::scoped(&state.pool))
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

/// Add an environment to a project, for someone the caller checked may
/// manage its settings. The first one, or one asked to be, becomes the
/// default. The caller records the audit row.
pub(crate) async fn create_in(
    state: &AppState,
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    request: EnvironmentRequest,
) -> Result<EnvironmentRow, AppError> {
    let make_default = request.make_default;
    let clean = clean(state, project_id, request).await?;
    if rewritten_on_the_way(clean.provider_environment_id.as_deref()) {
        return Err(AppError::Validation("providerEnvironmentId"));
    }
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
    .bind(actor_id)
    .fetch_one(&mut **transaction)
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
    .fetch_one(&mut **transaction)
    .await?;
    if make_default || !has_default {
        set_default(transaction, organization_id, project_id, id, actor_id).await?;
    }
    Ok(row)
}

/// Change an active environment (the request's expectedVersion must be
/// current), for someone the caller checked may manage the project's
/// settings. The caller records the audit row.
pub(crate) async fn update_in(
    state: &AppState,
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    environment_id: Uuid,
    request: EnvironmentRequest,
) -> Result<EnvironmentRow, AppError> {
    let expected_version = request
        .expected_version
        .ok_or(AppError::Validation("expectedVersion"))?;
    let make_default = request.make_default;
    let clean = clean(state, project_id, request).await?;
    let (current, current_target) = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT version, provider_environment_id FROM project_environments WHERE project_id = $1 AND id = $2 AND archived_at IS NULL FOR UPDATE",
    )
    .bind(project_id)
    .bind(environment_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("environment"))?;
    // One saved before this rule stays, so other edits still go through.
    if clean.provider_environment_id != current_target
        && rewritten_on_the_way(clean.provider_environment_id.as_deref())
    {
        return Err(AppError::Validation("providerEnvironmentId"));
    }
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
    .fetch_one(&mut **transaction)
    .await
    .map_err(name_conflict)?;
    if make_default {
        set_default(
            transaction,
            organization_id,
            project_id,
            environment_id,
            actor_id,
        )
        .await?;
    }
    Ok(row)
}

/// Archive an active environment; a project default that pointed at it
/// falls back to the workspace's. The caller checked who may, and records
/// the audit row.
pub(crate) async fn archive_in(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    organization_id: Uuid,
    project_id: Uuid,
    environment_id: Uuid,
) -> Result<(), AppError> {
    let archived = sqlx::query(
        "UPDATE project_environments SET archived_at = now(), updated_at = now(), version = version + 1 WHERE project_id = $1 AND id = $2 AND archived_at IS NULL",
    )
    .bind(project_id)
    .bind(environment_id)
    .execute(&mut **transaction)
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
    .fetch_one(&mut **transaction)
    .await?;
    if pointed_here {
        settings::reset(
            transaction,
            settings::Target::Project {
                organization_id,
                project_id,
            },
            "run.environment",
            None,
        )
        .await?;
    }
    Ok(())
}

/// Make an active environment the project's default. The caller checked
/// who may, and records the audit row.
pub(crate) async fn make_default_in(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    organization_id: Uuid,
    project_id: Uuid,
    environment_id: Uuid,
    actor_id: Uuid,
) -> Result<(), AppError> {
    active_environment(&mut **transaction, project_id, environment_id)
        .await?
        .ok_or(AppError::NotFound("environment"))?;
    set_default(
        transaction,
        organization_id,
        project_id,
        environment_id,
        actor_id,
    )
    .await
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
          AND home_path IS NOT NULL
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

/// Check an environment with the caller's own signed-in account for its
/// harness, and record the result on it. The caller checked who may.
pub(crate) async fn check_in(
    state: &AppState,
    actor_id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    environment_id: Uuid,
) -> Result<(bool, String, EnvironmentRow), AppError> {
    let environment = active_environment(&state.pool, project_id, environment_id)
        .await?
        .ok_or(AppError::NotFound("environment"))?;
    let account = own_account(&state.pool, organization_id, actor_id, &environment.harness).await?;
    let catalog = Harnesses::of(state, organization_id).await?;
    let manifest = catalog.manifest(&environment.harness);
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
        (Some(manifest), Some(_))
            if manifest.runs_on == RunLocation::Cloud
                && rewritten_on_the_way(environment.provider_environment_id.as_deref()) =>
        {
            (
                false,
                "This environment's ID reads like a password, so it can't be sent as it is. Change it in the environment's settings.".to_owned(),
            )
        }
        (Some(manifest), Some(account)) if manifest.runs_on == RunLocation::Cloud => {
            let provider_environment = environment
                .provider_environment_id
                .clone()
                .unwrap_or_default();
            let asked = crate::hosts::builtin::accounts::ask(
                state,
                organization_id,
                account.ask(AccountQuestion::CheckEnvironment {
                    environment: provider_environment.clone(),
                }),
            )
            .await?;
            let name = &manifest.display_name;
            match asked {
                Asked::Answered(AccountAnswer::Environment { found: true }) => (
                    true,
                    format!("{name} found {provider_environment} for your account."),
                ),
                Asked::Answered(AccountAnswer::Environment { found: false }) => (
                    false,
                    format!(
                        "{name} couldn't find {provider_environment} for your account. Check the ID or label in its environment settings."
                    ),
                ),
                // A harness with no environments of its own: signing in is
                // all it needs.
                Asked::Unsupported => (true, format!("Your {name} account is signed in.")),
                _ => (
                    false,
                    format!("{name} couldn't be asked about {provider_environment}; try again."),
                ),
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
    .fetch_one(crate::db::scoped(&state.pool))
    .await?;
    Ok((ok, message, row))
}

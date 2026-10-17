//! Provider accounts (S6): the sign-ins people bring to a workspace. An
//! account belongs to its owner, not to a project; each project the owner
//! runs it in has an execution profile pointing at it. Sign-in happens once
//! per account and every project that uses it sees the result.
//!
//! The older per-profile routes stay as thin aliases for the account behind
//! the profile.

use std::path::PathBuf;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, Subject,
        authorize,
    },
    profile_runtime::prepare_private_profile_directory,
    provider::{
        AccountHandle, AccountSummary, HarnessError, LoginCeremonyError, LoginCeremonySnapshot,
        LoginKind, LoginReservation, LoginStart, ProfileHealth, RateLimitSummary,
        catalog::run_catalog, manifest::AdapterManifest, run_config::RunConfig,
    },
};

const MAX_ACCOUNTS_PER_OWNER: i64 = 50;
const MAX_ACCOUNT_NAME_CHARS: usize = 120;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/provider-accounts",
            get(list_accounts).post(create_account),
        )
        .route(
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}",
            get(read_account).patch(update_account),
        )
        .route(
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/use",
            post(use_account),
        )
        .route(
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/browser",
            post(account_browser_login),
        )
        .route(
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/device-code",
            post(account_device_code_login),
        )
        .route(
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/api-key",
            post(account_api_key_login),
        )
        .route(
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/login/cancel",
            post(account_cancel_login),
        )
        .route(
            "/api/v1/projects/{project_id}/provider-accounts/{account_id}/logout",
            post(account_logout),
        )
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account",
            get(read_profile_account),
        )
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/browser",
            post(profile_browser_login),
        )
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/device-code",
            post(profile_device_code_login),
        )
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/api-key",
            post(profile_api_key_login),
        )
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/login/cancel",
            post(profile_cancel_login),
        )
        .route(
            "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account/logout",
            post(profile_logout),
        )
}

/// One account the caller owns, reached through a project they can manage
/// execution in.
#[derive(Clone, Debug, FromRow)]
pub(crate) struct AccountAccess {
    pub(crate) account_id: Uuid,
    pub(crate) organization_id: Uuid,
    pub(crate) adapter_kind: String,
    pub(crate) home_path: String,
    pub(crate) max_concurrent_runs: i16,
    pub(crate) status: String,
    pub(crate) account_state: String,
    /// The profile the request came through, if it used a profile route.
    pub(crate) profile_id: Option<Uuid>,
    /// That profile's own status (a profile can be disabled on its own).
    pub(crate) profile_status: Option<String>,
}

impl AccountAccess {
    fn handle(&self) -> AccountHandle {
        AccountHandle {
            id: self.account_id,
            home: PathBuf::from(&self.home_path),
            max_concurrent_runs: u16::try_from(self.max_concurrent_runs).unwrap_or(1),
        }
    }

    fn disabled(&self) -> bool {
        self.status == "disabled" || self.profile_status.as_deref() == Some("disabled")
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiKeyRequest {
    api_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateAccountRequest {
    adapter_kind: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateAccountRequest {
    expected_version: i64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    max_concurrent_runs: Option<u16>,
    #[serde(default)]
    run_defaults: Option<RunConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AccountCommandReference {
    account_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountStatusResponse {
    account_id: Uuid,
    profile_id: Option<Uuid>,
    account_state: String,
    provider_available: bool,
    account: Option<AccountIdentityView>,
    rate_limits: Option<RateLimitView>,
    ceremony: Option<LoginCeremonyView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountIdentityView {
    account_type: String,
    email: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitView {
    primary: Option<RateLimitWindowView>,
    secondary: Option<RateLimitWindowView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitWindowView {
    used_percent: u16,
    resets_at: Option<i64>,
    window_duration_minutes: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginCeremonyView {
    ceremony_id: Uuid,
    kind: &'static str,
    state: &'static str,
    expires_at_unix_seconds: u64,
    auth_url: Option<String>,
    verification_url: Option<String>,
    user_code: Option<String>,
}

#[derive(Debug, FromRow)]
struct AccountRow {
    id: Uuid,
    adapter_kind: String,
    name: String,
    status: String,
    account_state: String,
    max_concurrent_runs: i16,
    run_defaults: Value,
    last_health_at: Option<String>,
    last_failure_category: Option<String>,
    version: i64,
    profile_id: Option<Uuid>,
    preferred: Option<bool>,
    active_runs: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderAccountView {
    id: Uuid,
    adapter_kind: String,
    harness_name: Option<String>,
    name: String,
    status: String,
    account_state: String,
    max_concurrent_runs: u16,
    run_defaults: RunConfig,
    last_health_at: Option<String>,
    last_failure_category: Option<String>,
    version: i64,
    /// The profile that uses this account in the project, if any.
    profile_id: Option<Uuid>,
    /// Whether this project's runs use this account first.
    preferred: bool,
    active_runs: i64,
    supervisor: SupervisorView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorView {
    running: bool,
    circuit_open: bool,
    consecutive_failures: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderAccountsResponse {
    accounts: Vec<ProviderAccountView>,
}

#[derive(Debug, FromRow)]
struct ProjectContext {
    organization_id: Uuid,
    role: String,
    handle: String,
}

// ---------------------------------------------------------------------------
// Account management

async fn list_accounts(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ProviderAccountsResponse>, AppError> {
    let project = require_project_manager(&state, &principal, project_id).await?;
    let rows = sqlx::query_as::<_, AccountRow>(&account_select(
        "account.organization_id = $2 AND account.owner_actor_id = $3",
    ))
    .bind(project_id)
    .bind(project.organization_id)
    .bind(principal.actor_id)
    .fetch_all(&state.pool)
    .await?;
    let mut accounts = Vec::with_capacity(rows.len());
    for row in rows {
        accounts.push(account_view(&state, row).await);
    }
    Ok(Json(ProviderAccountsResponse { accounts }))
}

fn account_select(filter: &str) -> String {
    format!(
        r#"
        SELECT account.id, account.adapter_kind, account.name, account.status,
               account.account_state, account.max_concurrent_runs,
               account.run_defaults, account.last_health_at::text AS last_health_at,
               account.last_failure_category, account.version,
               profile.id AS profile_id, profile.preferred,
               (
                   SELECT count(*) FROM agent_runs run
                   JOIN execution_profiles run_profile
                     ON run_profile.id = run.execution_profile_id
                   WHERE run_profile.provider_account_id = account.id
                     AND run.state IN ('starting', 'working', 'blocked', 'recovering')
               ) AS active_runs
        FROM provider_accounts account
        LEFT JOIN execution_profiles profile
          ON profile.provider_account_id = account.id AND profile.project_id = $1
        WHERE {filter}
        ORDER BY account.created_at, account.id
        LIMIT {MAX_ACCOUNTS_PER_OWNER}
        "#
    )
}

async fn account_view(state: &AppState, row: AccountRow) -> ProviderAccountView {
    let health = state
        .supervisors
        .health(&row.adapter_kind, row.id)
        .await
        .unwrap_or(ProfileHealth::default());
    ProviderAccountView {
        harness_name: state
            .supervisors
            .manifest(&row.adapter_kind)
            .map(|manifest| manifest.display_name.clone()),
        id: row.id,
        adapter_kind: row.adapter_kind,
        name: row.name,
        status: row.status,
        account_state: row.account_state,
        max_concurrent_runs: u16::try_from(row.max_concurrent_runs).unwrap_or(1),
        run_defaults: RunConfig::from_value(&row.run_defaults),
        last_health_at: row.last_health_at,
        last_failure_category: row.last_failure_category,
        version: row.version,
        profile_id: row.profile_id,
        preferred: row.preferred.unwrap_or(false),
        active_runs: row.active_runs,
        supervisor: SupervisorView {
            running: health.process_running,
            circuit_open: health.circuit_open,
            consecutive_failures: health.consecutive_failures,
        },
    }
}

async fn fetch_account_view(
    state: &AppState,
    project_id: Uuid,
    account_id: Uuid,
) -> Result<ProviderAccountView, AppError> {
    let row = sqlx::query_as::<_, AccountRow>(&account_select("account.id = $2"))
        .bind(project_id)
        .bind(account_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound("provider_account"))?;
    Ok(account_view(state, row).await)
}

/// Adds an account for a harness. It starts signed out; signing in happens
/// through the login routes. It is not used in any project until `use`.
async fn create_account(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateAccountRequest>,
) -> Result<(StatusCode, Json<ProviderAccountView>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let request_hash = RequestHash::for_value(&request)?;
    let manifest = state
        .supervisors
        .manifest(&request.adapter_kind)
        .map(|manifest| (*manifest).clone())
        .ok_or(AppError::Validation("adapterKind"))?;
    let name = match request.name.as_deref().map(str::trim) {
        Some(name) => valid_account_name(name)?,
        None => manifest.display_name.clone(),
    };
    let project = require_project_manager(&state, &principal, project_id).await?;
    // Another workspace's registered harness is not one this one can use.
    if state
        .supervisors
        .manifest_in(project.organization_id, &request.adapter_kind)
        .is_none()
    {
        return Err(AppError::Validation("adapterKind"));
    }
    let scope =
        idempotency::resource_bound_scope("project", project_id, "provider_account_create")?;
    let mut transaction = state.pool.begin().await?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let reference: AccountCommandReference = replay.decode()?;
            transaction.commit().await?;
            let view = fetch_account_view(&state, project_id, reference.account_id).await?;
            return Ok((replay.status(), Json(view)));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let account_id = insert_account(
        &state,
        &mut transaction,
        project.organization_id,
        principal.actor_id,
        &manifest,
        &name,
    )
    .await?;
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::CREATED,
        &AccountCommandReference { account_id },
    )
    .await?;
    transaction.commit().await?;
    let view = fetch_account_view(&state, project_id, account_id).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// Creates an account with its own private home directory.
pub(crate) async fn insert_account(
    state: &AppState,
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    organization_id: Uuid,
    owner_actor_id: Uuid,
    manifest: &AdapterManifest,
    name: &str,
) -> Result<Uuid, AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "provider-accounts:{organization_id}:{owner_actor_id}"
        ))
        .execute(&mut **transaction)
        .await?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM provider_accounts WHERE organization_id = $1 AND owner_actor_id = $2",
    )
    .bind(organization_id)
    .bind(owner_actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if count >= MAX_ACCOUNTS_PER_OWNER {
        return Err(AppError::Conflict(
            "provider_account",
            "too many provider accounts",
        ));
    }
    let account_id = Uuid::now_v7();
    let home_key = format!("acct-{}", account_id.simple());
    let data_root = state.config.data_root.clone();
    let home_root = state.config.codex_home_root.clone();
    let home = tokio::task::spawn_blocking(move || {
        prepare_private_profile_directory(&data_root, &home_root, &home_key)
    })
    .await
    .map_err(|_| AppError::Internal("prepare provider account home task"))??;
    let home = home
        .to_str()
        .ok_or(AppError::Internal("provider account home path encoding"))?;
    sqlx::query(
        r#"
        INSERT INTO provider_accounts (
            id, organization_id, owner_actor_id, adapter_kind, name, home_path,
            max_concurrent_runs, status, account_state
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7,
            'authentication_required', 'authentication_required'
        )
        "#,
    )
    .bind(account_id)
    .bind(organization_id)
    .bind(owner_actor_id)
    .bind(&manifest.kind)
    .bind(name)
    .bind(home)
    .bind(i16::try_from(manifest.concurrency.default_per_account.max(1)).unwrap_or(1))
    .execute(&mut **transaction)
    .await?;
    Ok(account_id)
}

fn valid_account_name(name: &str) -> Result<String, AppError> {
    if name.is_empty()
        || name.chars().count() > MAX_ACCOUNT_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(AppError::Validation("name"));
    }
    Ok(name.to_owned())
}

async fn update_account(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, account_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateAccountRequest>,
) -> Result<Json<ProviderAccountView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let request_hash = RequestHash::for_value(&request)?;
    let access = require_account_owner(&state, &principal, project_id, account_id).await?;
    let manifest = state
        .supervisors
        .manifest(&access.adapter_kind)
        .map(|manifest| (*manifest).clone())
        .ok_or(AppError::Conflict(
            "provider_account",
            "this harness is not installed",
        ))?;
    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .map(valid_account_name)
        .transpose()?;
    let max_concurrent_runs = match request.max_concurrent_runs {
        Some(limit) if limit == 0 || limit > manifest.concurrency.max_per_account => {
            return Err(AppError::Validation("maxConcurrentRuns"));
        }
        Some(limit) => {
            Some(i16::try_from(limit).map_err(|_| AppError::Validation("maxConcurrentRuns"))?)
        }
        None => None,
    };
    let run_defaults = match request.run_defaults {
        Some(defaults) => {
            // Checked as if named at launch, so a saved default must be
            // pickable now even though stale ones fall back at run time.
            let catalog = run_catalog(&state.pool, access.organization_id, &manifest).await?;
            defaults
                .resolve(&RunConfig::default(), &manifest, &catalog)
                .map_err(|error| AppError::Validation(error.0))?;
            Some(
                serde_json::to_value(&defaults)
                    .map_err(|_| AppError::Internal("serialize run defaults"))?,
            )
        }
        None => None,
    };
    // A retried PATCH whose first response was lost replays instead of
    // failing the version check it already passed.
    let scope = idempotency::resource_bound_scope("provider_account", account_id, "update")?;
    let mut transaction = state.pool.begin().await?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            replay.decode::<AccountCommandReference>()?;
            transaction.commit().await?;
            return Ok(Json(
                fetch_account_view(&state, project_id, account_id).await?,
            ));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let updated = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE provider_accounts
        SET name = COALESCE($3, name),
            max_concurrent_runs = COALESCE($4, max_concurrent_runs),
            run_defaults = COALESCE($5, run_defaults),
            version = version + 1,
            updated_at = now()
        WHERE id = $1 AND version = $2
        RETURNING version
        "#,
    )
    .bind(account_id)
    .bind(request.expected_version)
    .bind(name)
    .bind(max_concurrent_runs)
    .bind(run_defaults)
    .fetch_optional(&mut *transaction)
    .await?;
    if updated.is_none() {
        let current_version =
            sqlx::query_scalar::<_, i64>("SELECT version FROM provider_accounts WHERE id = $1")
                .bind(account_id)
                .fetch_one(&state.pool)
                .await?;
        return Err(AppError::VersionConflict {
            resource: "provider_account",
            current_version,
        });
    }
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::OK,
        &AccountCommandReference { account_id },
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(
        fetch_account_view(&state, project_id, account_id).await?,
    ))
}

/// Uses one of the caller's accounts in this project, making it the one
/// their runs here pick first.
async fn use_account(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, account_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<ProviderAccountView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    // The project is part of the request, so reusing a key for another
    // project is refused rather than replayed.
    let request_hash = RequestHash::for_value(&serde_json::json!({ "projectId": project_id }))?;
    let access = require_account_owner(&state, &principal, project_id, account_id).await?;
    let project = require_project_manager(&state, &principal, project_id).await?;
    let scope = idempotency::resource_bound_scope("provider_account", account_id, "use")?;
    let mut transaction = state.pool.begin().await?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            replay.decode::<AccountCommandReference>()?;
            transaction.commit().await?;
            return Ok(Json(
                fetch_account_view(&state, project_id, account_id).await?,
            ));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    use_account_in_project(
        &state,
        &mut transaction,
        project_id,
        principal.actor_id,
        &project.handle,
        &access,
    )
    .await?;
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::OK,
        &AccountCommandReference { account_id },
    )
    .await?;
    transaction.commit().await?;
    crate::execution_setup::activate_pending_worker_for_account(&state.pool, account_id).await?;
    Ok(Json(
        fetch_account_view(&state, project_id, account_id).await?,
    ))
}

/// Makes sure the owner has a profile for `account` in the project, marks it
/// preferred, and sets up the project's provider connection and worker when
/// this is the first. Returns the profile id and whether it was created.
pub(crate) async fn use_account_in_project(
    state: &AppState,
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
    owner_actor_id: Uuid,
    owner_handle: &str,
    account: &AccountAccess,
) -> Result<(Uuid, bool), AppError> {
    let manifest = state
        .supervisors
        .manifest(&account.adapter_kind)
        .map(|manifest| (*manifest).clone())
        .ok_or(AppError::Conflict(
            "provider_account",
            "this harness is not installed",
        ))?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("execution-profile:{project_id}:{owner_actor_id}"))
        .execute(&mut **transaction)
        .await?;
    crate::execution_setup::lock_project_setup(transaction, project_id).await?;
    let existing = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM execution_profiles WHERE project_id = $1 AND provider_account_id = $2",
    )
    .bind(project_id)
    .bind(account.account_id)
    .fetch_optional(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE execution_profiles SET preferred = false, updated_at = now()
        WHERE project_id = $1 AND owner_actor_id = $2 AND preferred
          AND provider_account_id <> $3
        "#,
    )
    .bind(project_id)
    .bind(owner_actor_id)
    .bind(account.account_id)
    .execute(&mut **transaction)
    .await?;
    if let Some(profile_id) = existing {
        sqlx::query(
            "UPDATE execution_profiles SET preferred = true, updated_at = now() WHERE id = $1 AND NOT preferred",
        )
        .bind(profile_id)
        .execute(&mut **transaction)
        .await?;
        return Ok((profile_id, false));
    }
    let existing_worker_connection = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT worker.provider_connection_id
        FROM execution_workers worker
        WHERE worker.project_id = $1
        ORDER BY worker.created_at, worker.id
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let provider_connection_id =
        crate::execution_setup::ensure_provider_connection(transaction, project_id, &manifest)
            .await?;
    let profile_id = Uuid::now_v7();
    // Profile names are unique per project; a second account for the same
    // harness gets a short piece of its id. The project setup lock above
    // keeps this check and the insert together.
    let base = format!("{} · @{owner_handle}", manifest.display_name);
    let mut name: String = base.chars().take(MAX_ACCOUNT_NAME_CHARS).collect();
    let taken = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM execution_profiles WHERE project_id = $1 AND name = $2)",
    )
    .bind(project_id)
    .bind(&name)
    .fetch_one(&mut **transaction)
    .await?;
    if taken {
        let suffix = format!(" · {}", &account.account_id.simple().to_string()[24..]);
        name = base
            .chars()
            .take(MAX_ACCOUNT_NAME_CHARS - suffix.chars().count())
            .chain(suffix.chars())
            .collect();
    }
    sqlx::query(
        r#"
        INSERT INTO execution_profiles (
            id, project_id, provider_connection_id, owner_actor_id, name,
            provider_account_id, preferred
        )
        VALUES ($1, $2, $3, $4, $5, $6, true)
        "#,
    )
    .bind(profile_id)
    .bind(project_id)
    .bind(provider_connection_id)
    .bind(owner_actor_id)
    .bind(name)
    .bind(account.account_id)
    .execute(&mut **transaction)
    .await?;
    if existing_worker_connection.is_none() {
        crate::execution_setup::ensure_default_worker(
            transaction,
            project_id,
            provider_connection_id,
            profile_id,
            false,
        )
        .await?;
    }
    Ok((profile_id, true))
}

// ---------------------------------------------------------------------------
// Sign-in, by account or by profile

async fn read_account(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, account_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<AccountStatusResponse>, AppError> {
    let access = require_account_owner(&state, &principal, project_id, account_id).await?;
    account_status(&state, &principal, access).await
}

async fn read_profile_account(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<AccountStatusResponse>, AppError> {
    let access = require_profile_owner(&state, &principal, project_id, profile_id).await?;
    account_status(&state, &principal, access).await
}

async fn account_browser_login(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, account_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<LoginCeremonyView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let access = require_account_owner(&state, &principal, project_id, account_id).await?;
    start_login(
        &state,
        &principal,
        &headers,
        access,
        LoginKind::Browser,
        None,
    )
    .await
}

async fn account_device_code_login(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, account_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<LoginCeremonyView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let access = require_account_owner(&state, &principal, project_id, account_id).await?;
    start_login(
        &state,
        &principal,
        &headers,
        access,
        LoginKind::DeviceCode,
        None,
    )
    .await
}

async fn account_api_key_login(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, account_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<ApiKeyRequest>,
) -> Result<Json<LoginCeremonyView>, AppError> {
    let result = async {
        require_authenticated_write(&state, &headers, &principal)?;
        let access = require_account_owner(&state, &principal, project_id, account_id).await?;
        start_login(
            &state,
            &principal,
            &headers,
            access,
            LoginKind::ApiKey,
            Some(request.api_key.as_str()),
        )
        .await
    }
    .await;
    request.api_key.zeroize();
    result
}

async fn account_cancel_login(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, account_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<AccountStatusResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    IdempotencyKey::from_headers(&headers)?;
    let access = require_account_owner(&state, &principal, project_id, account_id).await?;
    cancel_login(&state, &principal, access).await
}

async fn account_logout(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, account_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<AccountStatusResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    IdempotencyKey::from_headers(&headers)?;
    let access = require_account_owner(&state, &principal, project_id, account_id).await?;
    logout(&state, access).await
}

async fn profile_browser_login(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<LoginCeremonyView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let access = require_profile_owner(&state, &principal, project_id, profile_id).await?;
    start_login(
        &state,
        &principal,
        &headers,
        access,
        LoginKind::Browser,
        None,
    )
    .await
}

async fn profile_device_code_login(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<LoginCeremonyView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let access = require_profile_owner(&state, &principal, project_id, profile_id).await?;
    start_login(
        &state,
        &principal,
        &headers,
        access,
        LoginKind::DeviceCode,
        None,
    )
    .await
}

async fn profile_api_key_login(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<ApiKeyRequest>,
) -> Result<Json<LoginCeremonyView>, AppError> {
    let result = async {
        require_authenticated_write(&state, &headers, &principal)?;
        let access = require_profile_owner(&state, &principal, project_id, profile_id).await?;
        start_login(
            &state,
            &principal,
            &headers,
            access,
            LoginKind::ApiKey,
            Some(request.api_key.as_str()),
        )
        .await
    }
    .await;
    request.api_key.zeroize();
    result
}

async fn profile_cancel_login(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<AccountStatusResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    IdempotencyKey::from_headers(&headers)?;
    let access = require_profile_owner(&state, &principal, project_id, profile_id).await?;
    cancel_login(&state, &principal, access).await
}

async fn profile_logout(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(_request): ApiJson<EmptyRequest>,
) -> Result<Json<AccountStatusResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    IdempotencyKey::from_headers(&headers)?;
    let access = require_profile_owner(&state, &principal, project_id, profile_id).await?;
    logout(&state, access).await
}

fn status_response(access: &AccountAccess, account_state: &str) -> AccountStatusResponse {
    AccountStatusResponse {
        account_id: access.account_id,
        profile_id: access.profile_id,
        account_state: account_state.to_owned(),
        provider_available: false,
        account: None,
        rate_limits: None,
        ceremony: None,
    }
}

async fn account_status(
    state: &AppState,
    principal: &AuthenticatedActor,
    access: AccountAccess,
) -> Result<Json<AccountStatusResponse>, AppError> {
    let account_id = access.account_id;
    let mut ceremony = state
        .supervisors
        .login_ceremony(account_id, principal.actor_id, principal.session_id)
        .await
        .map(login_ceremony_view);
    if access.disabled() {
        return Ok(Json(AccountStatusResponse {
            ceremony,
            ..status_response(&access, &access.account_state)
        }));
    }
    let provider = state
        .supervisors
        .acquire_control(&access.adapter_kind, access.handle())
        .await;
    let mut provider = match provider {
        Ok(provider) => provider,
        // Every control slot is busy with other account pages: that says
        // nothing about this account, so its stored state stands.
        Err(HarnessError::ControlCapacity) => {
            return Ok(Json(AccountStatusResponse {
                ceremony,
                ..status_response(&access, &access.account_state)
            }));
        }
        Err(_) => {
            persist_account_state(state, account_id, "unknown", "unavailable").await?;
            return Ok(Json(AccountStatusResponse {
                ceremony,
                ..status_response(&access, "unknown")
            }));
        }
    };
    let account = match provider.read_account().await {
        Ok(account) => account,
        Err(error) => {
            tracing::warn!(
                %account_id,
                error_type = std::any::type_name_of_val(&error),
                "provider account status is unavailable"
            );
            persist_account_state(state, account_id, "unknown", "unavailable").await?;
            return Ok(Json(AccountStatusResponse {
                ceremony,
                ..status_response(&access, "unknown")
            }));
        }
    };
    let (account_state, account_status) = if account.authenticated {
        ("authenticated", "available")
    } else {
        ("authentication_required", "authentication_required")
    };
    persist_account_state(state, account_id, account_state, account_status).await?;
    if account.authenticated {
        state.supervisors.clear_login(account_id, None).await;
        ceremony = None;
    }
    let rate_limits = if account.authenticated {
        provider.read_rate_limits().await.ok().map(rate_limit_view)
    } else {
        None
    };
    Ok(Json(AccountStatusResponse {
        account_id,
        profile_id: access.profile_id,
        account_state: account_state.to_owned(),
        provider_available: true,
        account: identity_view(account),
        rate_limits,
        ceremony,
    }))
}

async fn start_login(
    state: &AppState,
    principal: &AuthenticatedActor,
    headers: &HeaderMap,
    access: AccountAccess,
    kind: LoginKind,
    api_key: Option<&str>,
) -> Result<Json<LoginCeremonyView>, AppError> {
    let idempotency_key = IdempotencyKey::from_headers(headers)?;
    let account_id = access.account_id;
    if access.disabled() {
        return Err(AppError::Conflict(
            "provider_account",
            "account is disabled",
        ));
    }
    let supported = state
        .supervisors
        .manifest(&access.adapter_kind)
        .is_some_and(|manifest| {
            manifest
                .login_methods
                .iter()
                .any(|method| method.kind == kind)
        });
    if !supported {
        return Err(AppError::Conflict(
            "provider_account",
            "this harness does not support that sign-in method",
        ));
    }
    let reservation = state
        .supervisors
        .reserve_login(
            account_id,
            principal.actor_id,
            principal.session_id,
            idempotency_key.as_str(),
            kind,
        )
        .await
        .map_err(login_ceremony_error)?;
    let ceremony_id = match reservation {
        LoginReservation::Replay(snapshot) => return Ok(Json(login_ceremony_view(snapshot))),
        LoginReservation::Reserved(ceremony_id) => ceremony_id,
    };

    let provider = state
        .supervisors
        .acquire_control(&access.adapter_kind, access.handle())
        .await;
    let mut provider = match provider {
        Ok(provider) => provider,
        Err(error) => {
            state
                .supervisors
                .clear_login(account_id, Some(ceremony_id))
                .await;
            return Err(provider_error(account_id, "start login session", error));
        }
    };
    let response = match (kind, api_key) {
        (LoginKind::ApiKey, Some(api_key)) => provider.login_with_api_key(api_key).await,
        (LoginKind::ApiKey, None) => {
            state
                .supervisors
                .clear_login(account_id, Some(ceremony_id))
                .await;
            return Err(AppError::Validation("apiKey"));
        }
        (kind, _) => provider.start_login(kind).await,
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            state
                .supervisors
                .clear_login(account_id, Some(ceremony_id))
                .await;
            return Err(provider_error(account_id, "start login", error));
        }
    };
    let snapshot = state
        .supervisors
        .complete_login(account_id, ceremony_id, response)
        .await
        .map_err(login_ceremony_error)?;
    let (account_state, account_status) = if kind == LoginKind::ApiKey {
        ("authenticated", "available")
    } else {
        ("authentication_required", "authentication_required")
    };
    persist_account_state(state, account_id, account_state, account_status).await?;
    Ok(Json(login_ceremony_view(snapshot)))
}

async fn cancel_login(
    state: &AppState,
    principal: &AuthenticatedActor,
    access: AccountAccess,
) -> Result<Json<AccountStatusResponse>, AppError> {
    let account_id = access.account_id;
    let ceremony = state
        .supervisors
        .login_ceremony(account_id, principal.actor_id, principal.session_id)
        .await;
    let Some(ceremony) = ceremony else {
        persist_account_state(
            state,
            account_id,
            "authentication_required",
            "authentication_required",
        )
        .await?;
        return Ok(Json(AccountStatusResponse {
            provider_available: true,
            ..status_response(&access, "authentication_required")
        }));
    };
    let login_id = match ceremony.response.as_ref() {
        Some(LoginStart::Browser { login_id, .. })
        | Some(LoginStart::DeviceCode { login_id, .. }) => Some(login_id.clone()),
        Some(LoginStart::ApiKey) => None,
        None => return Err(AppError::Conflict("login_ceremony", "login is starting")),
    };
    if let Some(login_id) = login_id {
        let mut provider = state
            .supervisors
            .acquire_control(&access.adapter_kind, access.handle())
            .await
            .map_err(|error| provider_error(account_id, "cancel login session", error))?;
        provider
            .cancel_login(&login_id)
            .await
            .map_err(|error| provider_error(account_id, "cancel login", error))?;
    }
    state
        .supervisors
        .clear_login(account_id, Some(ceremony.ceremony_id))
        .await;
    persist_account_state(
        state,
        account_id,
        "authentication_required",
        "authentication_required",
    )
    .await?;
    Ok(Json(AccountStatusResponse {
        provider_available: true,
        ..status_response(&access, "authentication_required")
    }))
}

async fn logout(
    state: &AppState,
    access: AccountAccess,
) -> Result<Json<AccountStatusResponse>, AppError> {
    let account_id = access.account_id;
    persist_account_state(state, account_id, "logged_out", "authentication_required").await?;
    state
        .supervisors
        .shutdown_account(&access.adapter_kind, account_id)
        .await;
    state.supervisors.clear_login(account_id, None).await;
    // Always tell the harness, even when the account already reads logged
    // out: a retry after a failed or lost first attempt must still revoke
    // the harness's credentials.
    let mut provider = state
        .supervisors
        .acquire_control(&access.adapter_kind, access.handle())
        .await
        .map_err(|error| provider_error(account_id, "logout session", error))?;
    provider
        .logout()
        .await
        .map_err(|error| provider_error(account_id, "logout", error))?;
    drop(provider);
    state
        .supervisors
        .shutdown_account(&access.adapter_kind, account_id)
        .await;
    Ok(Json(status_response(&access, "logged_out")))
}

// ---------------------------------------------------------------------------
// Access

async fn require_project_manager(
    state: &AppState,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<ProjectContext, AppError> {
    let project = sqlx::query_as::<_, ProjectContext>(
        r#"
        SELECT project.organization_id, membership.role, membership.handle
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = $2
         AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
        WHERE project.id = $1 AND project.archived_at IS NULL
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("project"))?;
    if may_manage_execution(principal, project_id, &project.role) {
        Ok(project)
    } else {
        Err(AppError::NotFound("project"))
    }
}

fn may_manage_execution(principal: &AuthenticatedActor, project_id: Uuid, role: &str) -> bool {
    let role = match role {
        "owner" => MembershipRole::Owner,
        "admin" => MembershipRole::Admin,
        "member" => MembershipRole::Member,
        "viewer" => MembershipRole::Viewer,
        _ => return false,
    };
    principal.actor_kind == "human"
        && matches!(
            authorize(
                &Subject::new(
                    principal.actor_id,
                    ActorKind::Human,
                    true,
                    Some(Membership::project(project_id, role, true)),
                    None,
                ),
                Action::ExecutionProfileManage,
                Resource::Project(ProjectScope::project(project_id)),
            ),
            Decision::Allow
        )
}

/// The caller's own account in the workspace of a project they can manage
/// execution in. Someone else's account is indistinguishable from a missing
/// one.
pub(crate) async fn require_account_owner(
    state: &AppState,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    account_id: Uuid,
) -> Result<AccountAccess, AppError> {
    let project = require_project_manager(state, principal, project_id).await?;
    sqlx::query_as::<_, AccountAccess>(
        r#"
        SELECT account.id AS account_id, account.organization_id,
               account.adapter_kind, account.home_path,
               account.max_concurrent_runs, account.status,
               account.account_state,
               NULL::uuid AS profile_id, NULL::text AS profile_status
        FROM provider_accounts account
        WHERE account.id = $1
          AND account.organization_id = $2
          AND account.owner_actor_id = $3
        "#,
    )
    .bind(account_id)
    .bind(project.organization_id)
    .bind(principal.actor_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("provider_account"))
}

async fn require_profile_owner(
    state: &AppState,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    profile_id: Uuid,
) -> Result<AccountAccess, AppError> {
    #[derive(FromRow)]
    struct Row {
        #[sqlx(flatten)]
        access: AccountAccess,
        role: String,
    }
    let row = sqlx::query_as::<_, Row>(
        r#"
        SELECT account.id AS account_id, account.organization_id,
               account.adapter_kind, account.home_path,
               account.max_concurrent_runs, account.status,
               account.account_state,
               profile.id AS profile_id, profile.status AS profile_status,
               membership.role
        FROM execution_profiles profile
        JOIN provider_accounts account
          ON account.id = profile.provider_account_id
        JOIN projects project ON project.id = profile.project_id
        JOIN project_memberships membership
          ON membership.project_id = profile.project_id
         AND membership.actor_id = profile.owner_actor_id
         AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = profile.owner_actor_id
         AND organization_membership.enabled
        JOIN provider_connections connection
          ON connection.id = profile.provider_connection_id
         AND connection.project_id = profile.project_id
         AND connection.enabled
        WHERE profile.id = $1
          AND profile.project_id = $2
          AND profile.owner_actor_id = $3
          AND project.archived_at IS NULL
        "#,
    )
    .bind(profile_id)
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("execution_profile"))?;
    if may_manage_execution(principal, project_id, &row.role) {
        Ok(row.access)
    } else {
        Err(AppError::NotFound("execution_profile"))
    }
}

/// Records what the harness said about the account. A trigger copies it to
/// every profile that uses the account, in every project.
async fn persist_account_state(
    state: &AppState,
    account_id: Uuid,
    account_state: &'static str,
    account_status: &'static str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE provider_accounts
        SET account_state = $2,
            status = CASE WHEN status = 'disabled' THEN status ELSE $3 END,
            last_health_at = now(),
            last_failure_category = NULL,
            version = version + CASE
                WHEN account_state IS DISTINCT FROM $2
                  OR (status <> 'disabled' AND status IS DISTINCT FROM $3)
                THEN 1 ELSE 0 END,
            updated_at = CASE
                WHEN account_state IS DISTINCT FROM $2
                  OR (status <> 'disabled' AND status IS DISTINCT FROM $3)
                THEN now() ELSE updated_at END
        WHERE id = $1
        "#,
    )
    .bind(account_id)
    .bind(account_state)
    .bind(account_status)
    .execute(&state.pool)
    .await?;
    if account_status == "available" {
        crate::execution_setup::activate_pending_worker_for_account(&state.pool, account_id)
            .await?;
    }
    Ok(())
}

fn identity_view(account: AccountSummary) -> Option<AccountIdentityView> {
    account
        .account_type
        .map(|account_type| AccountIdentityView {
            account_type,
            email: account.email,
        })
}

fn rate_limit_view(rate_limits: RateLimitSummary) -> RateLimitView {
    let window = |value: crate::provider::RateLimitWindow| RateLimitWindowView {
        used_percent: value.used_percent,
        resets_at: value.resets_at,
        window_duration_minutes: value.window_duration_minutes,
    };
    RateLimitView {
        primary: rate_limits.primary.map(window),
        secondary: rate_limits.secondary.map(window),
    }
}

fn login_ceremony_view(ceremony: LoginCeremonySnapshot) -> LoginCeremonyView {
    let (state, auth_url, verification_url, user_code) = match ceremony.response {
        Some(LoginStart::Browser { auth_url, .. }) => ("pending", Some(auth_url), None, None),
        Some(LoginStart::DeviceCode {
            verification_url,
            user_code,
            ..
        }) => ("pending", None, Some(verification_url), Some(user_code)),
        Some(LoginStart::ApiKey) => ("completed", None, None, None),
        None => ("starting", None, None, None),
    };
    LoginCeremonyView {
        ceremony_id: ceremony.ceremony_id,
        kind: match ceremony.kind {
            LoginKind::Browser => "browser",
            LoginKind::DeviceCode => "device_code",
            LoginKind::ApiKey => "api_key",
        },
        state,
        expires_at_unix_seconds: ceremony.expires_at_unix_seconds,
        auth_url,
        verification_url,
        user_code,
    }
}

fn login_ceremony_error(error: LoginCeremonyError) -> AppError {
    match error {
        LoginCeremonyError::Active => {
            AppError::Conflict("login_ceremony", "another login is active")
        }
        LoginCeremonyError::Capacity => {
            AppError::Conflict("login_ceremony", "host login capacity is reached")
        }
        LoginCeremonyError::NotFound => AppError::NotFound("login_ceremony"),
    }
}

fn provider_error(
    account_id: Uuid,
    operation: &'static str,
    error: impl std::fmt::Debug,
) -> AppError {
    tracing::error!(
        %account_id,
        operation,
        error_type = std::any::type_name_of_val(&error),
        "provider account operation failed"
    );
    AppError::Conflict("provider_account", "the provider account operation failed")
}

/// An account by id inside a transaction, for callers that already checked
/// who owns it.
pub(crate) async fn account_access_in(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account_id: Uuid,
) -> Result<AccountAccess, AppError> {
    Ok(sqlx::query_as::<_, AccountAccess>(
        r#"
        SELECT account.id AS account_id, account.organization_id,
               account.adapter_kind, account.home_path,
               account.max_concurrent_runs, account.status,
               account.account_state,
               NULL::uuid AS profile_id, NULL::text AS profile_status
        FROM provider_accounts account
        WHERE account.id = $1
        "#,
    )
    .bind(account_id)
    .fetch_one(&mut **transaction)
    .await?)
}

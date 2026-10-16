use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    policy::{
        Action, ActorKind, Membership, MembershipRole, ProjectScope, Resource, Subject, authorize,
    },
    provider::{
        CODEX_KIND, ProfileHealth, SupervisorRegistry,
        catalog::{self, Catalog},
        manifest::{AdapterManifest, ModelOverrides},
    },
};

const MAX_EXECUTION_RESOURCES: i64 = 100;
const MAX_PROFILE_NAME_CHARS: usize = 120;
const MAX_DISPLAY_NAME_CHARS: usize = 120;
const MAX_HANDLE_CHARS: usize = 32;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{id}/execution-workers",
            get(list_execution_workers),
        )
        .route(
            "/api/v1/projects/{id}/execution-profiles",
            get(list_execution_profiles).post(register_personal_execution_profile),
        )
        .route("/api/v1/projects/{id}/harnesses", get(list_harnesses))
        .route(
            "/api/v1/projects/{id}/harnesses/{kind}/catalog/refresh",
            post(refresh_catalog),
        )
        .route(
            "/api/v1/projects/{id}/harnesses/{kind}/models/{model_id}",
            put(save_model_overrides).delete(remove_model),
        )
}

#[derive(Debug, FromRow)]
struct ProjectAccessRow {
    role: String,
    handle: String,
}

/// Registering a profile uses one of the caller's provider accounts in the
/// project. With no account named, it reuses their existing profile, or their
/// existing account of the harness, or creates one.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisterProfileRequest {
    #[serde(default)]
    provider_account_id: Option<Uuid>,
    #[serde(default)]
    adapter_kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ExecutionProfileCommandReference {
    profile_id: Uuid,
}

#[derive(Debug, FromRow)]
struct ExecutionWorkerRow {
    id: Uuid,
    version: i64,
    actor_id: Uuid,
    actor_kind: String,
    actor_status: String,
    display_name: String,
    handle: String,
    worker_enabled: bool,
    membership_enabled: bool,
    profile_status: Option<String>,
    profile_account_state: Option<String>,
    /// Whether the profile's connection is enabled and its harness installed.
    profile_usable: Option<bool>,
    profile_owner_actor_status: Option<String>,
    profile_owner_membership_enabled: Option<bool>,
    provider_kind: String,
    provider_enabled: bool,
    provider_version: i64,
    binary_version: String,
    capabilities: serde_json::Value,
}

#[derive(Debug, FromRow)]
struct ExecutionProfileRow {
    id: Uuid,
    name: String,
    version: i64,
    status: String,
    account_state: String,
    provider_account_id: Uuid,
    account_adapter_kind: String,
    max_concurrent_runs: i16,
    preferred: bool,
    last_health_at: Option<String>,
    last_failure_category: Option<String>,
    owner_actor_id: Uuid,
    owner_actor_kind: String,
    owner_display_name: String,
    owner_handle: String,
    owner_actor_status: String,
    owner_membership_enabled: bool,
    provider_kind: String,
    provider_enabled: bool,
    provider_version: i64,
    binary_version: String,
    capabilities: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionWorkersResponse {
    workers: Vec<ExecutionWorkerView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionWorkerView {
    id: Uuid,
    version: i64,
    actor: ExecutionActorView,
    enabled: bool,
    status: ExecutionWorkerStatus,
    provider: ProviderView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ExecutionWorkerStatus {
    Available,
    Unavailable,
    AuthenticationRequired,
    Disabled,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionProfilesResponse {
    profiles: Vec<ExecutionProfileView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionProfileView {
    id: Uuid,
    name: String,
    version: i64,
    status: ExecutionProfileStatus,
    account_state: AccountState,
    /// Only shown to the account's owner; managers see the profile, not the
    /// private account behind it.
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_account_id: Option<Uuid>,
    max_concurrent_runs: u16,
    preferred: bool,
    last_health_at: Option<String>,
    last_failure_category: Option<String>,
    owner: ExecutionActorView,
    available_for_new_runs: bool,
    supervisor: SupervisorHealthView,
    provider: ProviderView,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorHealthView {
    state: SupervisorState,
    consecutive_failures: u32,
    process_starts: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SupervisorState {
    Stopped,
    Ready,
    CircuitOpen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ExecutionProfileStatus {
    Available,
    Unavailable,
    AuthenticationRequired,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AccountState {
    Unknown,
    Authenticated,
    AuthenticationRequired,
    LoggedOut,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionActorView {
    id: Uuid,
    kind: &'static str,
    display_name: String,
    handle: String,
    status: ActorStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ActorStatus {
    Active,
    Disabled,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderView {
    kind: String,
    /// The harness's name, or `None` when this server has no adapter for it.
    display_name: Option<String>,
    enabled: bool,
    version: i64,
    binary_version: String,
    /// Whether the registered adapter speaks this binary version.
    supported: bool,
    capabilities: ProviderCapabilities,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderCapabilities {
    supports_streaming: bool,
    supports_commands: bool,
    supports_file_changes: bool,
    supports_approvals: bool,
    supports_questions: bool,
    supports_resume: bool,
    supports_interrupt: bool,
    supports_pull_request_creation: bool,
    supports_remote_execution: bool,
}

async fn list_execution_workers(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ExecutionWorkersResponse>, AppError> {
    let mut transaction = state.pool.begin().await?;
    require_project_action(
        &mut transaction,
        &principal,
        project_id,
        Action::ProjectView,
    )
    .await?;
    let rows = sqlx::query_as::<_, ExecutionWorkerRow>(
        r#"
        SELECT
            worker.id,
            worker.version,
            worker.actor_id,
            actor.kind AS actor_kind,
            actor.status AS actor_status,
            membership.display_name,
            membership.handle,
            worker.enabled AS worker_enabled,
            membership.enabled AS membership_enabled,
            profile.status AS profile_status,
            profile.account_state AS profile_account_state,
            profile.usable AS profile_usable,
            profile_owner.status AS profile_owner_actor_status,
            profile_owner_membership.enabled AS profile_owner_membership_enabled,
            connection.kind AS provider_kind,
            connection.enabled AS provider_enabled,
            connection.version AS provider_version,
            connection.binary_version,
            connection.capabilities
        FROM execution_workers worker
        JOIN actors actor ON actor.id = worker.actor_id
        JOIN project_memberships membership
          ON membership.project_id = worker.project_id
         AND membership.actor_id = worker.actor_id
         AND membership.role = 'worker'
        JOIN provider_connections connection
          ON connection.project_id = worker.project_id
         AND connection.id = worker.provider_connection_id
        LEFT JOIN LATERAL (
            SELECT candidate.*,
                   COALESCE(candidate_connection.enabled
                            AND candidate_account.adapter_kind = ANY($4), false) AS usable
            FROM execution_profiles candidate
            LEFT JOIN provider_connections candidate_connection
              ON candidate_connection.project_id = candidate.project_id
             AND candidate_connection.id = candidate.provider_connection_id
            LEFT JOIN provider_accounts candidate_account
              ON candidate_account.id = candidate.provider_account_id
            WHERE candidate.project_id = worker.project_id
              AND candidate.owner_actor_id = $2
            -- Any of the person's profiles can drive the worker, whichever
            -- harness it was set up with, so readiness looks at all of them,
            -- ranking first the ones a run could actually start with.
            ORDER BY
                (candidate.status = 'available'
                 AND candidate.account_state = 'authenticated'
                 AND COALESCE(candidate_connection.enabled
                              AND candidate_account.adapter_kind = ANY($4), false)) DESC,
                (candidate.status = 'available'
                 AND candidate.account_state = 'authenticated') DESC,
                candidate.preferred DESC,
                candidate.created_at,
                candidate.id
            LIMIT 1
        ) profile ON true
        LEFT JOIN actors profile_owner ON profile_owner.id = profile.owner_actor_id
        LEFT JOIN project_memberships profile_owner_membership
          ON profile_owner_membership.project_id = profile.project_id
         AND profile_owner_membership.actor_id = profile.owner_actor_id
        WHERE worker.project_id = $1
        ORDER BY lower(membership.handle), worker.id
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .bind(MAX_EXECUTION_RESOURCES)
    .bind(
        state
            .supervisors
            .manifests()
            .map(|manifest| manifest.kind.clone())
            .collect::<Vec<_>>(),
    )
    .fetch_all(&mut *transaction)
    .await?;

    let workers = rows
        .into_iter()
        .map(|row| execution_worker_view(&state.supervisors, row))
        .collect::<Result<Vec<_>, _>>()?;
    transaction.commit().await?;
    Ok(Json(ExecutionWorkersResponse { workers }))
}

async fn list_execution_profiles(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ExecutionProfilesResponse>, AppError> {
    let mut transaction = state.pool.begin().await?;
    let access = require_project_action(
        &mut transaction,
        &principal,
        project_id,
        Action::ExecutionProfileView,
    )
    .await?;
    let can_view_team_readiness = matches!(access.role.as_str(), "owner" | "admin");
    let rows = sqlx::query_as::<_, ExecutionProfileRow>(
        r#"
        SELECT
            profile.id,
            profile.name,
            profile.version,
            profile.status,
            profile.account_state,
            profile.provider_account_id,
            account.adapter_kind AS account_adapter_kind,
            account.max_concurrent_runs,
            profile.preferred,
            profile.last_health_at::text AS last_health_at,
            profile.last_failure_category,
            profile.owner_actor_id,
            owner.kind AS owner_actor_kind,
            owner_membership.display_name AS owner_display_name,
            owner_membership.handle AS owner_handle,
            owner.status AS owner_actor_status,
            owner_membership.enabled AS owner_membership_enabled,
            connection.kind AS provider_kind,
            connection.enabled AS provider_enabled,
            connection.version AS provider_version,
            connection.binary_version,
            connection.capabilities
        FROM execution_profiles profile
        JOIN provider_accounts account ON account.id = profile.provider_account_id
        JOIN actors owner ON owner.id = profile.owner_actor_id
        JOIN project_memberships owner_membership
          ON owner_membership.project_id = profile.project_id
         AND owner_membership.actor_id = profile.owner_actor_id
        JOIN provider_connections connection
          ON connection.project_id = profile.project_id
         AND connection.id = profile.provider_connection_id
        WHERE profile.project_id = $1
          AND ($3 OR profile.owner_actor_id = $2)
        ORDER BY lower(profile.name), profile.id
        LIMIT $4
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .bind(can_view_team_readiness)
    .bind(MAX_EXECUTION_RESOURCES)
    .fetch_all(&mut *transaction)
    .await?;

    let mut profiles = Vec::with_capacity(rows.len());
    for row in rows {
        let health = state
            .supervisors
            .health(&row.account_adapter_kind, row.provider_account_id)
            .await;
        profiles.push(execution_profile_view(
            &state.supervisors,
            row,
            health,
            principal.actor_id,
        )?);
    }
    transaction.commit().await?;
    Ok(Json(ExecutionProfilesResponse { profiles }))
}

async fn register_personal_execution_profile(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<RegisterProfileRequest>,
) -> Result<(StatusCode, Json<ExecutionProfileView>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let request_hash = RequestHash::for_value(&request)?;
    let scope =
        idempotency::resource_bound_scope("project", project_id, "execution_profile_register")?;
    // Naming an account checks it belongs to the caller before anything is
    // reserved.
    let named_account = match request.provider_account_id {
        Some(account_id) => Some(
            crate::account::require_account_owner(&state, &principal, project_id, account_id)
                .await?,
        ),
        None => None,
    };
    // A named account decides the harness; naming a different one is refused.
    let adapter_kind = match (&request.adapter_kind, &named_account) {
        (Some(kind), Some(account)) if kind != &account.adapter_kind => {
            return Err(AppError::Validation("adapterKind"));
        }
        (_, Some(account)) => account.adapter_kind.clone(),
        (Some(kind), None) => kind.clone(),
        (None, None) => CODEX_KIND.to_owned(),
    };
    let manifest = state
        .supervisors
        .manifest(&adapter_kind)
        .cloned()
        .ok_or(AppError::Validation("adapterKind"))?;
    let mut transaction = state.pool.begin().await?;
    let access = require_project_action(
        &mut transaction,
        &principal,
        project_id,
        Action::ExecutionProfileManage,
    )
    .await?;
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
            let reference: ExecutionProfileCommandReference = replay.decode()?;
            transaction.commit().await?;
            let profile = fetch_owned_execution_profile(
                &state,
                project_id,
                reference.profile_id,
                principal.actor_id,
            )
            .await?;
            return Ok((replay.status(), Json(profile)));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "execution-profile:{project_id}:{}",
            principal.actor_id
        ))
        .execute(&mut *transaction)
        .await?;

    let account = match named_account {
        Some(account) => Some(account),
        None => {
            let existing_profile_id = sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT profile.id
                FROM execution_profiles profile
                JOIN provider_accounts account ON account.id = profile.provider_account_id
                JOIN provider_connections connection
                  ON connection.id = profile.provider_connection_id
                 AND connection.project_id = profile.project_id
                 AND connection.enabled
                WHERE profile.project_id = $1
                  AND profile.owner_actor_id = $2
                  AND account.adapter_kind = $3
                ORDER BY profile.preferred DESC, profile.created_at, profile.id
                LIMIT 1
                "#,
            )
            .bind(project_id)
            .bind(principal.actor_id)
            .bind(&adapter_kind)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some(profile_id) = existing_profile_id {
                let reference = ExecutionProfileCommandReference { profile_id };
                idempotency::complete(&mut transaction, &reservation, StatusCode::OK, &reference)
                    .await?;
                transaction.commit().await?;
                let profile = fetch_owned_execution_profile(
                    &state,
                    project_id,
                    profile_id,
                    principal.actor_id,
                )
                .await?;
                return Ok((StatusCode::OK, Json(profile)));
            }
            None
        }
    };
    // Reuse the caller's existing account of this harness so signing in once
    // covers every project; otherwise create one.
    let account = match account {
        Some(account) => account,
        None => {
            let organization_id =
                sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                    .bind(project_id)
                    .fetch_one(&mut *transaction)
                    .await?;
            let existing_account_id = sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT id FROM provider_accounts
                WHERE organization_id = $1 AND owner_actor_id = $2 AND adapter_kind = $3
                  AND status <> 'disabled'
                ORDER BY (account_state = 'authenticated') DESC, created_at, id
                LIMIT 1
                "#,
            )
            .bind(organization_id)
            .bind(principal.actor_id)
            .bind(&manifest.kind)
            .fetch_optional(&mut *transaction)
            .await?;
            let account_id = match existing_account_id {
                Some(account_id) => account_id,
                None => {
                    let name = manifest.display_name.clone();
                    crate::account::insert_account(
                        &state,
                        &mut transaction,
                        organization_id,
                        principal.actor_id,
                        &manifest,
                        &name,
                    )
                    .await?
                }
            };
            crate::account::account_access_in(&mut transaction, account_id).await?
        }
    };
    let (profile_id, created) = crate::account::use_account_in_project(
        &state,
        &mut transaction,
        project_id,
        principal.actor_id,
        &access.handle,
        &account,
    )
    .await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let reference = ExecutionProfileCommandReference { profile_id };
    idempotency::complete(&mut transaction, &reservation, status, &reference).await?;
    transaction.commit().await?;
    crate::execution_setup::activate_pending_worker_for_account(&state.pool, account.account_id)
        .await?;
    let profile =
        fetch_owned_execution_profile(&state, project_id, profile_id, principal.actor_id).await?;
    Ok((status, Json(profile)))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessView {
    #[serde(flatten)]
    manifest: AdapterManifest,
    catalog: Catalog,
    /// Whether the caller may edit this workspace's catalog.
    can_edit_catalog: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessesResponse {
    harnesses: Vec<HarnessView>,
}

#[derive(Debug, FromRow)]
struct WorkspaceAccessRow {
    organization_id: Uuid,
    organization_role: String,
}

async fn workspace_access(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<WorkspaceAccessRow, AppError> {
    Ok(sqlx::query_as::<_, WorkspaceAccessRow>(
        r#"
        SELECT project.organization_id, membership.role AS organization_role
        FROM projects project
        JOIN organization_memberships membership
          ON membership.organization_id = project.organization_id
         AND membership.actor_id = $2
         AND membership.enabled
        WHERE project.id = $1
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_one(&mut **transaction)
    .await?)
}

fn may_edit_catalog(principal: &AuthenticatedActor, workspace: &WorkspaceAccessRow) -> bool {
    principal.actor_kind == "human"
        && matches!(workspace.organization_role.as_str(), "owner" | "admin")
}

async fn harness_view(
    state: &AppState,
    organization_id: Uuid,
    manifest: &AdapterManifest,
    can_edit_catalog: bool,
) -> Result<HarnessView, AppError> {
    Ok(HarnessView {
        catalog: catalog::catalog(&state.pool, organization_id, manifest).await?,
        manifest: manifest.clone(),
        can_edit_catalog,
    })
}

/// Every harness this server can run, with its manifest and this
/// workspace's model catalog (H3).
async fn list_harnesses(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<HarnessesResponse>, AppError> {
    let mut transaction = state.pool.begin().await?;
    require_project_action(
        &mut transaction,
        &principal,
        project_id,
        Action::ProjectView,
    )
    .await?;
    let workspace = workspace_access(&mut transaction, &principal, project_id).await?;
    transaction.commit().await?;
    let can_edit = may_edit_catalog(&principal, &workspace);
    let mut harnesses = Vec::new();
    for manifest in state.supervisors.manifests() {
        harnesses.push(harness_view(&state, workspace.organization_id, manifest, can_edit).await?);
    }
    Ok(Json(HarnessesResponse { harnesses }))
}

/// Asks the harness for its current models, through one of the caller's
/// signed-in accounts, and stores them. People's edits are kept.
async fn refresh_catalog(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, kind)): ApiPath<(Uuid, String)>,
) -> Result<Json<HarnessView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    IdempotencyKey::from_headers(&headers)?;
    let manifest = state
        .supervisors
        .manifest(&kind)
        .cloned()
        .ok_or(AppError::NotFound("harness"))?;
    if !manifest.features.model_listing {
        return Err(AppError::Conflict(
            "harness",
            "this harness can't list its models; add them by hand",
        ));
    }
    let mut transaction = state.pool.begin().await?;
    // Same boundary as the other catalog edits: the project is visible and
    // the caller may edit the workspace catalog.
    require_project_action(
        &mut transaction,
        &principal,
        project_id,
        Action::ProjectView,
    )
    .await?;
    let workspace = workspace_access(&mut transaction, &principal, project_id).await?;
    // A refresh replaces the workspace's provider-reported models for
    // everyone, so it is a catalog edit.
    if !may_edit_catalog(&principal, &workspace) {
        return Err(AppError::Forbidden);
    }
    let account = sqlx::query_as::<_, crate::provider::ProfileAccount>(
        r#"
        SELECT account.id AS account_id, account.adapter_kind, account.home_path,
               account.max_concurrent_runs
        FROM provider_accounts account
        LEFT JOIN execution_profiles profile
          ON profile.provider_account_id = account.id AND profile.project_id = $4
        WHERE account.organization_id = $1 AND account.owner_actor_id = $2
          AND account.adapter_kind = $3
          AND account.status = 'available' AND account.account_state = 'authenticated'
        ORDER BY profile.preferred DESC NULLS LAST, account.created_at, account.id
        LIMIT 1
        "#,
    )
    .bind(workspace.organization_id)
    .bind(principal.actor_id)
    .bind(&manifest.kind)
    .bind(project_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::Conflict(
        "harness",
        "sign in to an account for this harness first",
    ))?;
    transaction.commit().await?;
    let mut session = state
        .supervisors
        .acquire_control(&manifest.kind, account.handle())
        .await
        .map_err(|error| catalog_error(&manifest.kind, error))?;
    let models = session
        .list_models()
        .await
        .map_err(|error| catalog_error(&manifest.kind, error))?;
    drop(session);
    catalog::store_refresh(
        &state.pool,
        workspace.organization_id,
        &manifest.kind,
        &models,
    )
    .await?;
    let can_edit = may_edit_catalog(&principal, &workspace);
    Ok(Json(
        harness_view(&state, workspace.organization_id, &manifest, can_edit).await?,
    ))
}

fn catalog_error(kind: &str, error: crate::provider::HarnessError) -> AppError {
    tracing::warn!(
        adapter_kind = kind,
        error_type = std::any::type_name_of_val(&error),
        "harness model listing failed"
    );
    AppError::Conflict("harness", "the harness could not list its models")
}

fn valid_model_id(model_id: &str) -> Result<(), AppError> {
    if (1..=200).contains(&model_id.len())
        && model_id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
        })
    {
        Ok(())
    } else {
        Err(AppError::Validation("modelId"))
    }
}

async fn require_catalog_editor(
    state: &AppState,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<Uuid, AppError> {
    let mut transaction = state.pool.begin().await?;
    require_project_action(&mut transaction, principal, project_id, Action::ProjectView).await?;
    let workspace = workspace_access(&mut transaction, principal, project_id).await?;
    transaction.commit().await?;
    if may_edit_catalog(principal, &workspace) {
        Ok(workspace.organization_id)
    } else {
        Err(AppError::Forbidden)
    }
}

/// A workspace owner or admin edits one model (context size, prices, name,
/// reasoning levels, speeds, hiding it), or adds one by hand.
async fn save_model_overrides(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, kind, model_id)): ApiPath<(Uuid, String, String)>,
    ApiJson(overrides): ApiJson<ModelOverrides>,
) -> Result<Json<HarnessView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    IdempotencyKey::from_headers(&headers)?;
    valid_model_id(&model_id)?;
    overrides.validate().map_err(AppError::Validation)?;
    let manifest = state
        .supervisors
        .manifest(&kind)
        .cloned()
        .ok_or(AppError::NotFound("harness"))?;
    let organization_id = require_catalog_editor(&state, &principal, project_id).await?;
    catalog::save_overrides(
        &state.pool,
        organization_id,
        &manifest.kind,
        &model_id,
        &overrides,
        principal.actor_id,
    )
    .await?;
    Ok(Json(
        harness_view(&state, organization_id, &manifest, true).await?,
    ))
}

async fn remove_model(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, kind, model_id)): ApiPath<(Uuid, String, String)>,
) -> Result<Json<HarnessView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    IdempotencyKey::from_headers(&headers)?;
    valid_model_id(&model_id)?;
    let manifest = state
        .supervisors
        .manifest(&kind)
        .cloned()
        .ok_or(AppError::NotFound("harness"))?;
    let organization_id = require_catalog_editor(&state, &principal, project_id).await?;
    if !catalog::remove_model(&state.pool, organization_id, &manifest.kind, &model_id).await? {
        return Err(AppError::NotFound("harness_model"));
    }
    Ok(Json(
        harness_view(&state, organization_id, &manifest, true).await?,
    ))
}

async fn fetch_owned_execution_profile(
    state: &AppState,
    project_id: Uuid,
    profile_id: Uuid,
    owner_actor_id: Uuid,
) -> Result<ExecutionProfileView, AppError> {
    let row = sqlx::query_as::<_, ExecutionProfileRow>(
        r#"
        SELECT
            profile.id,
            profile.name,
            profile.version,
            profile.status,
            profile.account_state,
            profile.provider_account_id,
            account.adapter_kind AS account_adapter_kind,
            account.max_concurrent_runs,
            profile.preferred,
            profile.last_health_at::text AS last_health_at,
            profile.last_failure_category,
            profile.owner_actor_id,
            owner.kind AS owner_actor_kind,
            owner_membership.display_name AS owner_display_name,
            owner_membership.handle AS owner_handle,
            owner.status AS owner_actor_status,
            owner_membership.enabled AS owner_membership_enabled,
            connection.kind AS provider_kind,
            connection.enabled AS provider_enabled,
            connection.version AS provider_version,
            connection.binary_version,
            connection.capabilities
        FROM execution_profiles profile
        JOIN provider_accounts account ON account.id = profile.provider_account_id
        JOIN actors owner ON owner.id = profile.owner_actor_id
        JOIN project_memberships owner_membership
          ON owner_membership.project_id = profile.project_id
         AND owner_membership.actor_id = profile.owner_actor_id
        JOIN provider_connections connection
          ON connection.project_id = profile.project_id
         AND connection.id = profile.provider_connection_id
        WHERE profile.project_id = $1
          AND profile.id = $2
          AND profile.owner_actor_id = $3
        "#,
    )
    .bind(project_id)
    .bind(profile_id)
    .bind(owner_actor_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("execution_profile"))?;
    let health = state
        .supervisors
        .health(&row.account_adapter_kind, row.provider_account_id)
        .await;
    execution_profile_view(&state.supervisors, row, health, owner_actor_id)
}

async fn require_project_action(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: Action,
) -> Result<ProjectAccessRow, AppError> {
    let access = sqlx::query_as::<_, ProjectAccessRow>(
        r#"
        SELECT membership.role, membership.handle
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = $2
         AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
        WHERE project.id = $1
          AND project.archived_at IS NULL
        FOR SHARE OF project, membership, organization_membership
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("project"))?;

    let subject = Subject::new(
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
        true,
        Some(Membership::project(
            project_id,
            membership_role(&access.role)?,
            true,
        )),
        None,
    );
    if authorize(
        &subject,
        action,
        Resource::Project(ProjectScope::project(project_id)),
    )
    .is_allowed()
    {
        Ok(access)
    } else {
        // Read endpoints deliberately make missing, foreign, disabled, and
        // unauthorized Projects indistinguishable.
        Err(AppError::NotFound("project"))
    }
}

fn execution_worker_view(
    registry: &SupervisorRegistry,
    row: ExecutionWorkerRow,
) -> Result<ExecutionWorkerView, AppError> {
    let actor_status = parse_actor_status(&row.actor_status)?;
    let profile_owner_status = row
        .profile_owner_actor_status
        .as_deref()
        .map(parse_actor_status)
        .transpose()?;
    let profile_status = row
        .profile_status
        .as_deref()
        .map(parse_profile_status)
        .transpose()?;
    let account_state = row
        .profile_account_state
        .as_deref()
        .map(parse_account_state)
        .transpose()?;
    let status = if !row.worker_enabled
        || !row.membership_enabled
        || actor_status == ActorStatus::Disabled
        || profile_owner_status == Some(ActorStatus::Disabled)
        || row.profile_owner_membership_enabled == Some(false)
        || profile_status == Some(ExecutionProfileStatus::Disabled)
    {
        ExecutionWorkerStatus::Disabled
    } else if profile_status.is_none()
        || profile_status == Some(ExecutionProfileStatus::AuthenticationRequired)
        || matches!(
            account_state,
            Some(AccountState::AuthenticationRequired | AccountState::LoggedOut)
        )
    {
        ExecutionWorkerStatus::AuthenticationRequired
    } else if !row.provider_enabled
        || registry.manifest(&row.provider_kind).is_none()
        || profile_status == Some(ExecutionProfileStatus::Unavailable)
        || row.profile_usable == Some(false)
        || account_state == Some(AccountState::Unknown)
    {
        ExecutionWorkerStatus::Unavailable
    } else {
        ExecutionWorkerStatus::Available
    };

    Ok(ExecutionWorkerView {
        id: row.id,
        version: positive_version(row.version)?,
        actor: execution_actor_view(
            row.actor_id,
            &row.actor_kind,
            row.display_name,
            row.handle,
            actor_status,
        )?,
        enabled: row.worker_enabled,
        status,
        provider: provider_view(
            registry,
            &row.provider_kind,
            row.provider_enabled,
            row.provider_version,
            &row.binary_version,
            &row.capabilities,
        )?,
    })
}

fn execution_profile_view(
    registry: &SupervisorRegistry,
    row: ExecutionProfileRow,
    supervisor_health: Option<ProfileHealth>,
    viewer_actor_id: Uuid,
) -> Result<ExecutionProfileView, AppError> {
    let status = parse_profile_status(&row.status)?;
    let account_state = parse_account_state(&row.account_state)?;
    let owner_status = parse_actor_status(&row.owner_actor_status)?;
    let available_for_new_runs = status == ExecutionProfileStatus::Available
        && account_state == AccountState::Authenticated
        && row.provider_enabled
        && registry.manifest(&row.provider_kind).is_some()
        && owner_status == ActorStatus::Active
        && row.owner_membership_enabled;

    Ok(ExecutionProfileView {
        id: row.id,
        name: bounded_identity(row.name, MAX_PROFILE_NAME_CHARS, "Execution Profile name")?,
        version: positive_version(row.version)?,
        status,
        account_state,
        provider_account_id: (row.owner_actor_id == viewer_actor_id)
            .then_some(row.provider_account_id),
        max_concurrent_runs: u16::try_from(row.max_concurrent_runs)
            .ok()
            .filter(|value| *value >= 1)
            .ok_or(AppError::Internal(
                "stored provider account concurrency limit",
            ))?,
        preferred: row.preferred,
        last_health_at: row.last_health_at,
        last_failure_category: bounded_failure_category(row.last_failure_category)?,
        owner: execution_actor_view(
            row.owner_actor_id,
            &row.owner_actor_kind,
            row.owner_display_name,
            row.owner_handle,
            owner_status,
        )?,
        available_for_new_runs,
        supervisor: supervisor_health_view(supervisor_health),
        provider: provider_view(
            registry,
            &row.provider_kind,
            row.provider_enabled,
            row.provider_version,
            &row.binary_version,
            &row.capabilities,
        )?,
    })
}

fn supervisor_health_view(health: Option<ProfileHealth>) -> SupervisorHealthView {
    let Some(health) = health else {
        return SupervisorHealthView {
            state: SupervisorState::Stopped,
            consecutive_failures: 0,
            process_starts: 0,
        };
    };
    SupervisorHealthView {
        state: if health.circuit_open {
            SupervisorState::CircuitOpen
        } else if health.process_running {
            SupervisorState::Ready
        } else {
            SupervisorState::Stopped
        },
        consecutive_failures: health.consecutive_failures,
        process_starts: health.process_starts,
    }
}

fn execution_actor_view(
    id: Uuid,
    kind: &str,
    display_name: String,
    handle: String,
    status: ActorStatus,
) -> Result<ExecutionActorView, AppError> {
    let kind = match kind {
        "human" => "human",
        "external_agent" => "external_agent",
        "execution_worker" => "execution_worker",
        "integration" => "integration",
        "system" => "system",
        _ => return Err(AppError::Internal("stored execution Actor kind")),
    };
    Ok(ExecutionActorView {
        id,
        kind,
        display_name: bounded_identity(
            display_name,
            MAX_DISPLAY_NAME_CHARS,
            "execution Actor display name",
        )?,
        handle: bounded_identity(handle, MAX_HANDLE_CHARS, "execution Actor handle")?,
        status,
    })
}

/// A provider connection as people see it. A harness this server has no
/// adapter for, or a binary version outside its range, shows as unsupported
/// rather than failing the whole list.
fn provider_view(
    registry: &SupervisorRegistry,
    kind: &str,
    enabled: bool,
    version: i64,
    binary_version: &str,
    capabilities: &serde_json::Value,
) -> Result<ProviderView, AppError> {
    let manifest = registry.manifest(kind);
    Ok(ProviderView {
        kind: kind.to_owned(),
        display_name: manifest.map(|manifest| manifest.display_name.clone()),
        enabled,
        version: positive_version(version)?,
        binary_version: binary_version.to_owned(),
        supported: manifest.is_some_and(|manifest| manifest.versions.contains(binary_version)),
        capabilities: ProviderCapabilities::from_json(capabilities),
    })
}

impl ProviderCapabilities {
    fn from_json(value: &serde_json::Value) -> Self {
        let flag = |name: &str| value.get(name).and_then(serde_json::Value::as_bool) == Some(true);
        Self {
            supports_streaming: flag("supports_streaming"),
            supports_commands: flag("supports_commands"),
            supports_file_changes: flag("supports_file_changes"),
            supports_approvals: flag("supports_approvals"),
            supports_questions: flag("supports_questions"),
            supports_resume: flag("supports_resume"),
            supports_interrupt: flag("supports_interrupt"),
            supports_pull_request_creation: flag("supports_pull_request_creation"),
            supports_remote_execution: flag("supports_remote_execution"),
        }
    }
}

fn bounded_identity(
    value: String,
    max_chars: usize,
    invariant: &'static str,
) -> Result<String, AppError> {
    if value.is_empty() || value.chars().count() > max_chars || value.chars().any(char::is_control)
    {
        Err(AppError::Internal(invariant))
    } else {
        Ok(value)
    }
}

fn bounded_failure_category(value: Option<String>) -> Result<Option<String>, AppError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let mut chars = value.chars();
    if value.len() > 64
        || !chars
            .next()
            .is_some_and(|character| character.is_ascii_lowercase())
        || !chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
    {
        return Err(AppError::Internal(
            "stored Execution Profile failure category",
        ));
    }
    Ok(Some(value))
}

fn positive_version(version: i64) -> Result<i64, AppError> {
    (version > 0)
        .then_some(version)
        .ok_or(AppError::Internal("stored execution resource version"))
}

fn parse_profile_status(value: &str) -> Result<ExecutionProfileStatus, AppError> {
    match value {
        "available" => Ok(ExecutionProfileStatus::Available),
        "unavailable" => Ok(ExecutionProfileStatus::Unavailable),
        "authentication_required" => Ok(ExecutionProfileStatus::AuthenticationRequired),
        "disabled" => Ok(ExecutionProfileStatus::Disabled),
        _ => Err(AppError::Internal("stored Execution Profile status")),
    }
}

fn parse_account_state(value: &str) -> Result<AccountState, AppError> {
    match value {
        "unknown" => Ok(AccountState::Unknown),
        "authenticated" => Ok(AccountState::Authenticated),
        "authentication_required" => Ok(AccountState::AuthenticationRequired),
        "logged_out" => Ok(AccountState::LoggedOut),
        _ => Err(AppError::Internal("stored Execution Profile account state")),
    }
}

fn parse_actor_status(value: &str) -> Result<ActorStatus, AppError> {
    match value {
        "active" => Ok(ActorStatus::Active),
        "disabled" => Ok(ActorStatus::Disabled),
        _ => Err(AppError::Internal("stored execution Actor status")),
    }
}

fn actor_kind(value: &str) -> Result<ActorKind, AppError> {
    match value {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("stored Actor kind")),
    }
}

fn membership_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("stored membership role")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        ActorStatus, ProviderCapabilities, bounded_failure_category, bounded_identity,
        execution_actor_view,
    };

    #[test]
    fn capability_projection_is_fixed_and_discards_private_provider_fields() {
        let capabilities = ProviderCapabilities::from_json(&json!({
            "supports_streaming": true,
            "supports_commands": "yes",
            "credential": "must-not-escape",
            "raw_provider_error": "must-not-escape",
            "future_private_capability": true
        }));

        assert!(capabilities.supports_streaming);
        assert!(!capabilities.supports_commands);
        let serialized = serde_json::to_string(&capabilities).expect("serialize capabilities");
        assert!(!serialized.contains("credential"));
        assert!(!serialized.contains("provider_error"));
        assert!(!serialized.contains("future_private_capability"));
    }

    #[test]
    fn failure_categories_are_bounded_safe_identifiers() {
        assert_eq!(
            bounded_failure_category(Some("process_start_failed".to_owned()))
                .expect("safe category"),
            Some("process_start_failed".to_owned())
        );
        assert!(bounded_failure_category(Some("raw error: token=secret".to_owned())).is_err());
        assert!(bounded_failure_category(Some("x".repeat(65))).is_err());
    }

    #[test]
    fn actor_identity_projection_matches_database_character_bounds() {
        let multibyte_name = "é".repeat(120);
        assert_eq!(
            bounded_identity(multibyte_name.clone(), 120, "name").expect("valid name"),
            multibyte_name
        );
        assert!(bounded_identity("x".repeat(121), 120, "name").is_err());
        assert!(bounded_identity("unsafe\nname".to_owned(), 120, "name").is_err());

        let actor = execution_actor_view(
            Uuid::nil(),
            "system",
            "Project service".to_owned(),
            "project-service".to_owned(),
            ActorStatus::Active,
        )
        .expect("supported service-owned profile Actor");
        assert_eq!(actor.kind, "system");
    }
}

use std::path::PathBuf;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    git_runtime::{
        RegisteredRepository, RepositoryPreparationMode, expected_repository_mirror_path,
        prepare_repository, prepare_repository_with_token, validate_registered_ref,
    },
    github::repository_git_token,
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    policy::{
        Action, ActorKind, Membership, Permission, ProjectScope, Resource, RoleGrant, Subject,
        authorize,
    },
};

const MAX_REPOSITORIES: i64 = 100;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/repositories",
            get(list_repositories).post(register_repository),
        )
        .route(
            "/api/v1/projects/{project_id}/repositories/{repository_id}/prepare",
            post(prepare_registered_repository),
        )
        .route(
            "/api/v1/projects/{project_id}/repositories/{repository_id}/archive",
            post(archive_repository),
        )
}

#[derive(Debug, FromRow)]
struct ProjectAccess {
    role: String,
    role_permissions: Option<Vec<String>>,
}

#[derive(Clone, Debug, FromRow)]
struct RepositoryRow {
    id: Uuid,
    github_repository_id: Option<i64>,
    owner: String,
    name: String,
    clone_url: String,
    default_branch: String,
    mirror_path: String,
    legacy_mirror_path: Option<String>,
    enabled: bool,
    trusted: bool,
    version: i64,
    preparation_state: String,
    preparation_failure_category: Option<String>,
    archived_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryListResponse {
    repositories: Vec<RepositoryView>,
    capabilities: RepositoryCapabilities,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegisterRepositoryRequest {
    github_repository_id: i64,
    owner: String,
    name: String,
    clone_url: String,
    default_branch: String,
    trust_acknowledged: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchiveRepositoryRequest {
    expected_version: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RepositoryCommandReference {
    repository_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryView {
    id: Uuid,
    github_repository_id: i64,
    owner: String,
    name: String,
    clone_url: String,
    default_branch: String,
    enabled: bool,
    trusted: bool,
    version: i64,
    preparation_state: String,
    preparation_failure_category: Option<String>,
    archived_at: Option<String>,
    capabilities: RepositoryCapabilities,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryCapabilities {
    manage: bool,
}

async fn list_repositories(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<RepositoryListResponse>, AppError> {
    let mut transaction = state.pool.begin().await?;
    let role = require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::RepositoryView,
    )
    .await?;
    let rows = sqlx::query_as::<_, RepositoryRow>(
        r#"
        SELECT id, github_repository_id, owner, name, clone_url,
               default_branch, mirror_path, legacy_mirror_path, enabled, trusted, version,
               preparation_state, preparation_failure_category,
               archived_at::text AS archived_at
        FROM repositories
        WHERE project_id = $1
        ORDER BY archived_at NULLS FIRST, lower(owner), lower(name), id
        LIMIT $2
        "#,
    )
    .bind(project_id)
    .bind(MAX_REPOSITORIES)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let manage = role_allows_manage(role);
    Ok(Json(RepositoryListResponse {
        repositories: rows
            .into_iter()
            .map(|row| repository_view(row, manage))
            .collect::<Result<_, _>>()?,
        capabilities: RepositoryCapabilities { manage },
    }))
}

async fn register_repository(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<RegisterRepositoryRequest>,
) -> Result<(StatusCode, Json<RepositoryView>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let request = validate_registration(request)?;
    let request_hash = RequestHash::for_value(&request)?;
    let scope = idempotency::resource_bound_scope("project", project_id, "repository_register")?;
    let mut transaction = state.pool.begin().await?;
    let role = require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::RepositoryManage,
    )
    .await?;
    // The answer is the repository (owner, name, clone URL, branch): only
    // for someone who also sees code here.
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::RepositoryView,
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
            let reference: RepositoryCommandReference = replay.decode()?;
            let row = fetch_repository(&mut transaction, project_id, reference.repository_id)
                .await?
                .ok_or(AppError::NotFound("repository"))?;
            transaction.commit().await?;
            return Ok((
                replay.status(),
                Json(repository_view(row, role_allows_manage(role))?),
            ));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let repository_id = insert_repository(&state, &mut transaction, project_id, &request).await?;
    let reference = RepositoryCommandReference { repository_id };
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::CREATED,
        &reference,
    )
    .await?;
    transaction.commit().await?;

    let mut transaction = state.pool.begin().await?;
    let row = fetch_repository(&mut transaction, project_id, repository_id)
        .await?
        .ok_or(AppError::NotFound("repository"))?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(repository_view(row, role_allows_manage(role))?),
    ))
}

/// Add a trusted repository row to a project. The caller has checked the
/// person may manage repositories and validated the request.
async fn insert_repository(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    request: &RegisterRepositoryRequest,
) -> Result<Uuid, AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "repository:{project_id}:{}:{}",
            request.owner.to_ascii_lowercase(),
            request.name.to_ascii_lowercase()
        ))
        .execute(&mut **transaction)
        .await?;
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM repositories WHERE project_id = $1 AND (lower(owner) = lower($2) AND lower(name) = lower($3) OR github_repository_id = $4))",
    )
    .bind(project_id)
    .bind(&request.owner)
    .bind(&request.name)
    .bind(request.github_repository_id)
    .fetch_one(&mut **transaction)
    .await?
    {
        return Err(AppError::Conflict(
            "repository",
            "repository is already registered",
        ));
    }

    let repository_id = Uuid::now_v7();
    let mirror_path = expected_repository_mirror_path(&state.config.data_root, repository_id)?;
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, github_repository_id, owner, name, clone_url,
            default_branch, mirror_path, enabled, trusted, preparation_state
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, true, true, 'pending')
        "#,
    )
    .bind(repository_id)
    .bind(project_id)
    .bind(request.github_repository_id)
    .bind(&request.owner)
    .bind(&request.name)
    .bind(&request.clone_url)
    .bind(&request.default_branch)
    .bind(mirror_path.to_string_lossy().as_ref())
    .execute(&mut **transaction)
    .await?;
    Ok(repository_id)
}

/// Register a GitHub repository picked from an installation the workspace
/// claimed. Same checks as typing it in, with the clone URL derived.
pub(crate) async fn register_picked_repository(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    github_repository_id: i64,
    owner: &str,
    name: &str,
    default_branch: &str,
) -> Result<Uuid, AppError> {
    let request = validate_registration(RegisterRepositoryRequest {
        github_repository_id,
        owner: owner.to_owned(),
        name: name.to_owned(),
        clone_url: format!("https://github.com/{owner}/{name}.git"),
        default_branch: default_branch.to_owned(),
        trust_acknowledged: true,
    })?;
    insert_repository(state, transaction, project_id, &request).await
}

async fn prepare_registered_repository(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, repository_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<EmptyRequest>,
) -> Result<Json<RepositoryView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let scope = idempotency::resource_bound_scope("repository", repository_id, "prepare")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let role = require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::RepositoryManage,
    )
    .await?;
    // The answer is the repository (owner, name, clone URL, branch): only
    // for someone who also sees code here.
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::RepositoryView,
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
            let reference: RepositoryCommandReference = replay.decode()?;
            let row = fetch_repository(&mut transaction, project_id, reference.repository_id)
                .await?
                .ok_or(AppError::NotFound("repository"))?;
            transaction.commit().await?;
            return Ok(Json(repository_view(row, role_allows_manage(role))?));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let admission_root = state.config.data_root.clone();
    let admission_quota = state.config.data_quota_bytes;
    let admission_free = state.config.minimum_free_bytes;
    tokio::task::spawn_blocking(move || {
        crate::storage_admission::require_growth(&admission_root, admission_quota, admission_free)
    })
    .await
    .map_err(|_| AppError::Internal("join storage admission"))??;
    let row = fetch_repository(&mut transaction, project_id, repository_id)
        .await?
        .filter(|row| row.archived_at.is_none() && row.enabled && row.trusted)
        .ok_or(AppError::NotFound("repository"))?;
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::OK,
        &RepositoryCommandReference { repository_id },
    )
    .await?;
    transaction.commit().await?;
    let row = run_preparation(&state, project_id, repository_id, row).await?;
    Ok(Json(repository_view(row, role_allows_manage(role))?))
}

async fn run_preparation(
    state: &AppState,
    project_id: Uuid,
    repository_id: Uuid,
    row: RepositoryRow,
) -> Result<RepositoryRow, AppError> {
    let registered = RegisteredRepository {
        id: repository_id,
        clone_url: row.clone_url,
        default_branch: row.default_branch,
        mirror_path: PathBuf::from(row.mirror_path),
    };
    let preparation_mode = if row.legacy_mirror_path.is_some() {
        RepositoryPreparationMode::VerifyImportedMirror
    } else {
        RepositoryPreparationMode::RefreshOrClone
    };
    let data_root = state.config.data_root.clone();
    let prepared = match preparation_mode {
        RepositoryPreparationMode::VerifyImportedMirror => {
            matches!(
                tokio::task::spawn_blocking(move || {
                    prepare_repository(data_root.as_path(), &registered, preparation_mode)
                })
                .await,
                Ok(Ok(_))
            )
        }
        RepositoryPreparationMode::RefreshOrClone => {
            match repository_git_token(state, project_id, repository_id).await {
                Ok(token) => matches!(
                    tokio::task::spawn_blocking(move || {
                        prepare_repository_with_token(
                            data_root.as_path(),
                            &registered,
                            preparation_mode,
                            &token,
                        )
                    })
                    .await,
                    Ok(Ok(_))
                ),
                Err(_) => false,
            }
        }
    };
    if !prepared {
        tracing::warn!(%project_id, %repository_id, "repository preparation failed");
    }
    let mut transaction = state.pool.begin().await?;
    let row =
        apply_preparation_result(&mut transaction, project_id, repository_id, prepared).await?;
    transaction.commit().await?;
    Ok(row)
}

/// Clone or refresh a repository without holding up the request that added
/// it; the repository list shows how it went.
pub(crate) fn prepare_in_background(state: AppState, project_id: Uuid, repository_id: Uuid) {
    tokio::spawn(async move {
        let result = async {
            let admission_root = state.config.data_root.clone();
            let admission_quota = state.config.data_quota_bytes;
            let admission_free = state.config.minimum_free_bytes;
            tokio::task::spawn_blocking(move || {
                crate::storage_admission::require_growth(
                    &admission_root,
                    admission_quota,
                    admission_free,
                )
            })
            .await
            .map_err(|_| AppError::Internal("join storage admission"))??;
            let mut transaction = state.pool.begin().await?;
            let row = fetch_repository(&mut transaction, project_id, repository_id)
                .await?
                .filter(|row| row.archived_at.is_none() && row.enabled && row.trusted)
                .ok_or(AppError::NotFound("repository"))?;
            transaction.commit().await?;
            run_preparation(&state, project_id, repository_id, row).await
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(
                %project_id,
                %repository_id,
                error_category = error.log_category(),
                "background repository preparation failed"
            );
        }
    });
}

/// A repository as the repositories API shows it.
pub(crate) async fn repository_json(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    repository_id: Uuid,
    manage: bool,
) -> Result<serde_json::Value, AppError> {
    let row = fetch_repository(transaction, project_id, repository_id)
        .await?
        .ok_or(AppError::NotFound("repository"))?;
    serde_json::to_value(repository_view(row, manage)?)
        .map_err(|_| AppError::Internal("repository view"))
}

async fn archive_repository(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    headers: HeaderMap,
    ApiPath((project_id, repository_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<ArchiveRepositoryRequest>,
) -> Result<Json<RepositoryView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("expectedVersion"));
    }
    let scope = idempotency::resource_bound_scope("repository", repository_id, "archive")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let role = require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::RepositoryManage,
    )
    .await?;
    // The answer is the repository (owner, name, clone URL, branch): only
    // for someone who also sees code here.
    require_project_access(
        &mut transaction,
        &principal,
        project_id,
        Action::RepositoryView,
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
            let reference: RepositoryCommandReference = replay.decode()?;
            let row = fetch_repository(&mut transaction, project_id, reference.repository_id)
                .await?
                .ok_or(AppError::NotFound("repository"))?;
            transaction.commit().await?;
            return Ok(Json(repository_view(row, role_allows_manage(role))?));
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    archive_in(
        &mut transaction,
        principal.actor_id,
        project_id,
        repository_id,
        request.expected_version,
    )
    .await?;
    let reference = RepositoryCommandReference { repository_id };
    idempotency::complete(&mut transaction, &reservation, StatusCode::OK, &reference).await?;
    let row = fetch_repository(&mut transaction, project_id, repository_id)
        .await?
        .ok_or(AppError::NotFound("repository"))?;
    transaction.commit().await?;
    Ok(Json(repository_view(row, role_allows_manage(role))?))
}

/// Archive a repository of the project (refused while a run uses it), for
/// someone the caller checked may manage repositories there.
pub(crate) async fn archive_in(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
    repository_id: Uuid,
    expected_version: i64,
) -> Result<(), AppError> {
    // The database refuses to archive a repository with an unfinished run;
    // checked first so the person hears why. Only this project's repository
    // counts, so another project's runs never show through.
    let has_active_run = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agent_runs run
            JOIN repositories repository ON repository.id = run.repository_id
            WHERE repository.id = $1 AND repository.project_id = $2
              AND run.state IN (
                  'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
              )
        )
        "#,
    )
    .bind(repository_id)
    .bind(project_id)
    .fetch_one(&mut **transaction)
    .await?;
    if has_active_run {
        return Err(AppError::RepositoryHasActiveRuns);
    }
    let updated = sqlx::query(
        r#"
        UPDATE repositories
        SET enabled = false,
            archived_at = now(),
            archived_by_actor_id = $4,
            version = version + 1,
            updated_at = now()
        WHERE id = $1 AND project_id = $2 AND version = $3
          AND archived_at IS NULL
        "#,
    )
    .bind(repository_id)
    .bind(project_id)
    .bind(expected_version)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        let current = fetch_repository(transaction, project_id, repository_id)
            .await?
            .ok_or(AppError::NotFound("repository"))?;
        return Err(AppError::VersionConflict {
            resource: "repository",
            current_version: current.version,
        });
    }
    Ok(())
}

/// Prepare (clone or refresh) an enabled repository of the project on this
/// server, for someone the caller checked may manage repositories there.
pub(crate) async fn prepare_in(
    state: &AppState,
    project_id: Uuid,
    repository_id: Uuid,
) -> Result<(), AppError> {
    let admission_root = state.config.data_root.clone();
    let admission_quota = state.config.data_quota_bytes;
    let admission_free = state.config.minimum_free_bytes;
    tokio::task::spawn_blocking(move || {
        crate::storage_admission::require_growth(&admission_root, admission_quota, admission_free)
    })
    .await
    .map_err(|_| AppError::Internal("join storage admission"))??;
    let mut transaction = state.pool.begin().await?;
    let row = fetch_repository(&mut transaction, project_id, repository_id)
        .await?
        .filter(|row| row.archived_at.is_none() && row.enabled && row.trusted)
        .ok_or(AppError::NotFound("repository"))?;
    transaction.commit().await?;
    run_preparation(state, project_id, repository_id, row).await?;
    Ok(())
}

/// Whether this role may manage repositories (for a view's capabilities).
pub(crate) fn may_manage(role: RoleGrant) -> bool {
    role_allows_manage(role)
}

async fn apply_preparation_result(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    repository_id: Uuid,
    prepared: bool,
) -> Result<RepositoryRow, AppError> {
    sqlx::query(
        r#"
        UPDATE repositories
        SET preparation_state = CASE WHEN $3 THEN 'ready' ELSE 'failed' END,
            prepared_at = CASE WHEN $3 THEN now() ELSE NULL END,
            preparation_failure_category = CASE WHEN $3 THEN NULL ELSE 'git_prepare_failed' END,
            version = version + 1,
            updated_at = now()
        WHERE id = $1 AND project_id = $2 AND archived_at IS NULL
          AND preparation_state IN ('pending', 'ready', 'failed')
        "#,
    )
    .bind(repository_id)
    .bind(project_id)
    .bind(prepared)
    .execute(&mut **transaction)
    .await?;
    fetch_repository(transaction, project_id, repository_id)
        .await?
        .ok_or(AppError::NotFound("repository"))
}

async fn fetch_repository(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    repository_id: Uuid,
) -> Result<Option<RepositoryRow>, AppError> {
    Ok(sqlx::query_as::<_, RepositoryRow>(
        r#"
        SELECT id, github_repository_id, owner, name, clone_url,
               default_branch, mirror_path, legacy_mirror_path, enabled, trusted, version,
               preparation_state, preparation_failure_category,
               archived_at::text AS archived_at
        FROM repositories
        WHERE id = $1 AND project_id = $2
        "#,
    )
    .bind(repository_id)
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn require_project_access(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: Action,
) -> Result<RoleGrant, AppError> {
    require_project_access_as(
        transaction,
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
        project_id,
        action,
    )
    .await
}

/// `require_project_access` for an actor, such as a person through MCP.
/// Returns their role in the project when `action` is allowed there.
pub(crate) async fn require_project_access_as(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    kind: ActorKind,
    project_id: Uuid,
    action: Action,
) -> Result<RoleGrant, AppError> {
    let access = sqlx::query_as::<_, ProjectAccess>(
        r#"
        SELECT membership.role, workspace_role_permissions(membership.role_id) AS role_permissions
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
        FOR SHARE OF project, membership, organization_membership
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("project"))?;
    let role = RoleGrant::stored(&access.role, access.role_permissions.as_deref())?;
    let subject = Subject::new(
        actor_id,
        kind,
        true,
        Some(Membership::project(project_id, role, true)),
        None,
    );
    if authorize(
        &subject,
        action,
        Resource::Project(ProjectScope::project(project_id)),
    )
    .is_allowed()
    {
        Ok(role)
    } else {
        Err(AppError::Forbidden)
    }
}

fn validate_registration(
    mut request: RegisterRepositoryRequest,
) -> Result<RegisterRepositoryRequest, AppError> {
    request.owner = request.owner.trim().to_owned();
    request.name = request.name.trim().to_owned();
    request.clone_url = request.clone_url.trim().to_owned();
    request.default_branch = request.default_branch.trim().to_owned();
    if request.github_repository_id <= 0 {
        return Err(AppError::Validation("githubRepositoryId"));
    }
    if !request.trust_acknowledged {
        return Err(AppError::Validation("trustAcknowledged"));
    }
    if !valid_github_component(&request.owner, false)
        || !valid_github_component(&request.name, true)
    {
        return Err(AppError::Validation("repositoryIdentity"));
    }
    let expected = format!("https://github.com/{}/{}.git", request.owner, request.name);
    if request.clone_url != expected || request.clone_url.contains('@') {
        return Err(AppError::Validation("cloneUrl"));
    }
    validate_registered_ref(&request.default_branch)?;
    Ok(request)
}

fn valid_github_component(value: &str, repository_name: bool) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || byte == b'-'
                || (repository_name && matches!(byte, b'.' | b'_'))
        })
}

fn repository_view(row: RepositoryRow, manage: bool) -> Result<RepositoryView, AppError> {
    if !matches!(
        row.preparation_state.as_str(),
        "pending" | "ready" | "failed"
    ) || row
        .preparation_failure_category
        .as_ref()
        .is_some_and(|value| {
            value.len() > 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
    {
        return Err(AppError::Internal("invalid repository state"));
    }
    let github_repository_id = row
        .github_repository_id
        .filter(|value| *value > 0)
        .ok_or(AppError::Internal("repository identity is unavailable"))?;
    Ok(RepositoryView {
        id: row.id,
        github_repository_id,
        owner: row.owner,
        name: row.name,
        clone_url: row.clone_url,
        default_branch: row.default_branch,
        enabled: row.enabled,
        trusted: row.trusted,
        version: row.version,
        preparation_state: row.preparation_state,
        preparation_failure_category: row.preparation_failure_category,
        archived_at: row.archived_at,
        capabilities: RepositoryCapabilities { manage },
    })
}

fn role_allows_manage(role: RoleGrant) -> bool {
    role.permissions().contains(Permission::CodeManage)
}

fn actor_kind(value: &str) -> Result<ActorKind, AppError> {
    match value {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("invalid actor kind")),
    }
}

#[cfg(test)]
mod tests {
    use super::{RegisterRepositoryRequest, validate_registration};

    fn valid() -> RegisterRepositoryRequest {
        RegisterRepositoryRequest {
            github_repository_id: 123,
            owner: "Mischa-dev".to_owned(),
            name: "agent-workspace".to_owned(),
            clone_url: "https://github.com/Mischa-dev/agent-workspace.git".to_owned(),
            default_branch: "main".to_owned(),
            trust_acknowledged: true,
        }
    }

    #[test]
    fn repository_registration_rejects_ambiguous_or_credentialed_identity() {
        assert!(validate_registration(valid()).is_ok());
        for mutate in [
            |request: &mut RegisterRepositoryRequest| request.owner = "../owner".to_owned(),
            |request: &mut RegisterRepositoryRequest| {
                request.clone_url =
                    "https://token@github.com/Mischa-dev/agent-workspace.git".to_owned();
            },
            |request: &mut RegisterRepositoryRequest| {
                request.default_branch = "-upload-pack".to_owned()
            },
            |request: &mut RegisterRepositoryRequest| request.trust_acknowledged = false,
        ] {
            let mut request = valid();
            mutate(&mut request);
            assert!(validate_registration(request).is_err());
        }
    }
}

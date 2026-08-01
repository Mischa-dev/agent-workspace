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
        prepare_repository, validate_registered_ref,
    },
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    policy::{
        Action, ActorKind, Membership, MembershipRole, ProjectScope, Resource, Subject, authorize,
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
        Action::ProjectView,
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
        Action::ProjectManage,
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
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "repository:{project_id}:{}:{}",
            request.owner.to_ascii_lowercase(),
            request.name.to_ascii_lowercase()
        ))
        .execute(&mut *transaction)
        .await?;
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM repositories WHERE project_id = $1 AND (lower(owner) = lower($2) AND lower(name) = lower($3) OR github_repository_id = $4))",
    )
    .bind(project_id)
    .bind(&request.owner)
    .bind(&request.name)
    .bind(request.github_repository_id)
    .fetch_one(&mut *transaction)
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
    .execute(&mut *transaction)
    .await?;
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
        Action::ProjectManage,
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
    let preparation = tokio::task::spawn_blocking(move || {
        prepare_repository(data_root.as_path(), &registered, preparation_mode)
    })
    .await;
    let prepared = matches!(preparation, Ok(Ok(_)));
    if !prepared {
        tracing::warn!(%project_id, %repository_id, "repository preparation failed");
    }
    let mut transaction = state.pool.begin().await?;
    let row =
        apply_preparation_result(&mut transaction, project_id, repository_id, prepared).await?;
    transaction.commit().await?;
    Ok(Json(repository_view(row, role_allows_manage(role))?))
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
        Action::ProjectManage,
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
    .bind(request.expected_version)
    .bind(principal.actor_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        let current = fetch_repository(&mut transaction, project_id, repository_id)
            .await?
            .ok_or(AppError::NotFound("repository"))?;
        return Err(AppError::VersionConflict {
            resource: "repository",
            current_version: current.version,
        });
    }
    let reference = RepositoryCommandReference { repository_id };
    idempotency::complete(&mut transaction, &reservation, StatusCode::OK, &reference).await?;
    let row = fetch_repository(&mut transaction, project_id, repository_id)
        .await?
        .ok_or(AppError::NotFound("repository"))?;
    transaction.commit().await?;
    Ok(Json(repository_view(row, role_allows_manage(role))?))
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
) -> Result<MembershipRole, AppError> {
    let access = sqlx::query_as::<_, ProjectAccess>(
        r#"
        SELECT membership.role
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = $2
         AND membership.enabled
        WHERE project.id = $1 AND project.archived_at IS NULL
        FOR SHARE OF project, membership
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("project"))?;
    let role = membership_role(&access.role)?;
    let subject = Subject::new(
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
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

fn role_allows_manage(role: MembershipRole) -> bool {
    matches!(role, MembershipRole::Owner | MembershipRole::Admin)
}

fn membership_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("invalid membership role")),
    }
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

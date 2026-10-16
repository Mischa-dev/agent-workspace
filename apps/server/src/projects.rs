use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header::LOCATION},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, Subject,
        authorize,
    },
};

const MAX_PROJECT_NAME_BYTES: usize = 120;
const MAX_SEARCH_BYTES: usize = 120;
const MAX_FILTER_BYTES: usize = 50_000;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 200;
const PROJECT_CREATE_SCOPE: &str = "projects.create";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/projects", get(list_projects).post(create_project))
        .route("/api/v1/projects/{id}", get(get_project))
        .route("/api/v1/projects/{id}/archive", post(archive_project))
        .route(
            "/api/v1/projects/{id}/navigation",
            get(get_navigation).put(update_navigation),
        )
        .route("/api/v1/projects/{id}/switch", post(switch_project))
}

#[derive(Debug, FromRow)]
struct ProjectRow {
    id: Uuid,
    organization_id: Uuid,
    project_key: String,
    name: String,
    role: String,
    version: i64,
    archived_at: Option<String>,
    location_kind: Option<String>,
    conversation_id: Option<Uuid>,
    context_drawer_open: Option<bool>,
    filters: Option<serde_json::Value>,
    pinned_at: Option<String>,
    last_opened_at: Option<String>,
    navigation_version: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectsResponse {
    projects: Vec<ProjectView>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    id: Uuid,
    organization_id: Uuid,
    key: String,
    name: String,
    archived_at: Option<String>,
    role: String,
    version: i64,
    capabilities: ProjectCapabilities,
    navigation: NavigationView,
    attention_count: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectCapabilities {
    view: bool,
    manage: bool,
    archive: bool,
    navigation_write: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigationView {
    location: NavigationLocation,
    href: String,
    context_drawer_open: bool,
    filters: serde_json::Value,
    pinned: bool,
    last_opened_at: Option<String>,
    version: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NavigationLocation {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectListQuery {
    q: Option<String>,
    limit: Option<u16>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateProjectRequest {
    organization_id: Uuid,
    key: String,
    name: String,
    /// A project template in the same workspace to start from.
    #[serde(default)]
    template_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchiveProjectRequest {
    expected_version: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateNavigationRequest {
    location: NavigationLocation,
    expected_version: i64,
    context_drawer_open: Option<bool>,
    filters: Option<serde_json::Value>,
    pinned: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SwitchProjectRequest {
    expected_version: i64,
}

#[derive(Debug, FromRow)]
struct OrganizationMembershipRow {
    role: String,
}

#[derive(Debug, FromRow)]
struct ProjectMembershipRow {
    id: Uuid,
    role: String,
    version: i64,
    archived_at: Option<String>,
}

#[derive(Debug, FromRow)]
struct NavigationStateRow {
    context_drawer_open: bool,
    filters: serde_json::Value,
    pinned: bool,
    version: i64,
}

#[derive(Debug, FromRow)]
struct IdempotencyRow {
    request_hash: Vec<u8>,
    response_status: Option<i32>,
    response_body: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct IdempotentProjectReference {
    id: Uuid,
}

async fn list_projects(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<ProjectListQuery>,
) -> Result<Json<ProjectsResponse>, AppError> {
    let limit = i64::from(query.limit.unwrap_or(50).clamp(1, 100));
    let search = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if search.is_some_and(|value| value.len() > MAX_SEARCH_BYTES) {
        return Err(AppError::Validation("project search is too long"));
    }
    if search.is_some() {
        state
            .rate_limits
            .check_expensive_read(principal.session_id)?;
    }
    let search_pattern = search.map(|value| format!("%{value}%"));
    let rows = sqlx::query_as::<_, ProjectRow>(
        r#"
        SELECT
            p.id,
            p.organization_id,
            p.project_key,
            p.name,
            pm.role,
            p.version,
            p.archived_at::text AS archived_at,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN 'project_overview'
                ELSE pns.location_kind
            END AS location_kind,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN NULL
                ELSE pns.conversation_id
            END AS conversation_id,
            pns.context_drawer_open,
            pns.filters,
            pns.pinned_at::text AS pinned_at,
            pns.last_opened_at::text AS last_opened_at,
            pns.version AS navigation_version
        FROM project_memberships pm
        JOIN projects p ON p.id = pm.project_id
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = pm.actor_id
         AND om.enabled
        LEFT JOIN project_navigation_states pns
          ON pns.project_id = p.id AND pns.actor_id = pm.actor_id
        LEFT JOIN conversations navc
          ON navc.project_id = p.id
         AND navc.id = pns.conversation_id
         AND navc.kind = pns.location_kind
         AND pns.location_kind IN ('channel', 'thread')
         AND navc.archived_at IS NULL
         AND NOT EXISTS (
             SELECT 1
             FROM conversations nav_parent
             WHERE navc.kind = 'thread'
               AND nav_parent.project_id = navc.project_id
               AND nav_parent.id = navc.parent_channel_id
               AND nav_parent.archived_at IS NOT NULL
         )
        WHERE pm.actor_id = $1
          AND pm.enabled
          AND p.archived_at IS NULL
          AND p.kind = 'standard'
          AND (
              $2::text IS NULL
              OR p.name ILIKE $2
              OR p.project_key ILIKE $2
          )
        ORDER BY
            -- An exact key comes first, so `/KEY-12` finds its Project before the limit.
            (upper(p.project_key) = upper($4)) IS TRUE DESC,
            (pns.pinned_at IS NOT NULL) DESC,
            pns.pinned_at ASC NULLS LAST,
            pns.last_opened_at DESC NULLS LAST,
            lower(p.name),
            p.id
        LIMIT $3
        "#,
    )
    .bind(principal.actor_id)
    .bind(search_pattern)
    .bind(limit)
    .bind(search)
    .fetch_all(&state.pool)
    .await?;
    let projects = rows
        .into_iter()
        .map(project_view)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ProjectsResponse { projects }))
}

async fn create_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<CreateProjectRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let idempotency_key = idempotency_key(&headers)?;
    let key = normalize_project_key(&request.key)?;
    let name = normalize_project_name(&request.name)?;
    let request_hash: [u8; 32] = Sha256::digest(
        serde_json::to_vec(&serde_json::json!({
            "organizationId": request.organization_id,
            "key": key,
            "name": name,
            "templateId": request.template_id,
        }))
        .map_err(|_| AppError::Internal("serialize Project command"))?,
    )
    .into();

    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "{}:{PROJECT_CREATE_SCOPE}:{idempotency_key}",
            principal.actor_id
        ))
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "DELETE FROM idempotency_keys WHERE actor_id = $1 AND scope = $2 AND key = $3 AND expires_at <= now()",
    )
    .bind(principal.actor_id)
    .bind(PROJECT_CREATE_SCOPE)
    .bind(&idempotency_key)
    .execute(&mut *transaction)
    .await?;
    let membership = sqlx::query_as::<_, OrganizationMembershipRow>(
        r#"
        SELECT role
        FROM organization_memberships
        WHERE organization_id = $1 AND actor_id = $2 AND enabled
        FOR UPDATE
        "#,
    )
    .bind(request.organization_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("organization"))?;
    authorize_organization_create(&principal, request.organization_id, &membership.role)?;

    if let Some(existing) = sqlx::query_as::<_, IdempotencyRow>(
        r#"
        SELECT request_hash, response_status, response_body
        FROM idempotency_keys
        WHERE actor_id = $1 AND scope = $2 AND key = $3
          AND expires_at > now()
        FOR UPDATE
        "#,
    )
    .bind(principal.actor_id)
    .bind(PROJECT_CREATE_SCOPE)
    .bind(&idempotency_key)
    .fetch_optional(&mut *transaction)
    .await?
    {
        if existing.request_hash != request_hash {
            return Err(AppError::Conflict("idempotency_key", "payload mismatch"));
        }
        let status = existing
            .response_status
            .and_then(|value| u16::try_from(value).ok())
            .and_then(|value| StatusCode::from_u16(value).ok())
            .ok_or(AppError::Conflict("idempotency_key", "command in progress"))?;
        let reference: IdempotentProjectReference = serde_json::from_value(
            existing
                .response_body
                .ok_or(AppError::Conflict("idempotency_key", "command in progress"))?,
        )
        .map_err(|_| AppError::Internal("stored idempotent Project reference"))?;
        let row =
            fetch_project_row_in_transaction(&mut transaction, principal.actor_id, reference.id)
                .await?
                .ok_or(AppError::NotFound("project"))?;
        if let Err(error) = authorize_project_action(&principal, &row, Action::ProjectView) {
            return match error {
                AppError::Forbidden => Err(AppError::NotFound("project")),
                error => Err(error),
            };
        }
        let project = project_view(row)?;
        transaction.commit().await?;
        return project_response(status, project);
    }

    sqlx::query(
        r#"
        INSERT INTO idempotency_keys (
            id, actor_id, scope, key, request_hash, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, now() + interval '24 hours')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(PROJECT_CREATE_SCOPE)
    .bind(&idempotency_key)
    .bind(request_hash.to_vec())
    .execute(&mut *transaction)
    .await?;

    let project_id = Uuid::now_v7();
    let insert = sqlx::query(
        r#"
        INSERT INTO projects (
            id, organization_id, project_key, name
        )
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(project_id)
    .bind(request.organization_id)
    .bind(&key)
    .bind(&name)
    .execute(&mut *transaction)
    .await;
    if let Err(error) = insert {
        if database_constraint(&error) == Some("projects_organization_key_unique") {
            return Err(AppError::ProjectKeyConflict);
        }
        return Err(error.into());
    }
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'owner', $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(principal.actor_id)
    .bind(&principal.display_name)
    .bind(membership_handle(&principal.login_name))
    .execute(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO project_counters (project_id, next_work_item_number) VALUES ($1, 1)")
        .bind(project_id)
        .execute(&mut *transaction)
        .await?;
    crate::work_items::ensure_default_workflow(&mut transaction, project_id).await?;
    let applied_template = match request.template_id {
        Some(template_id) => Some(
            crate::work_items::project_templates::apply_to_new_project(
                &mut transaction,
                &principal,
                crate::work_items::project_templates::creator_actor(&principal, project_id)?,
                request.organization_id,
                template_id,
            )
            .await?,
        ),
        None => None,
    };
    sqlx::query(
        r#"
        INSERT INTO project_instructions (project_id, content)
        VALUES ($1, '')
        "#,
    )
    .bind(project_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO project_navigation_states (
            project_id, actor_id, location_kind, context_drawer_open,
            filters, last_opened_at, version
        )
        VALUES ($1, $2, 'project_overview', false, '{}'::jsonb, now(), 1)
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .execute(&mut *transaction)
    .await?;

    let project = ProjectView {
        id: project_id,
        organization_id: request.organization_id,
        key,
        name,
        archived_at: None,
        role: "owner".to_owned(),
        version: 1,
        capabilities: ProjectCapabilities {
            view: true,
            manage: true,
            archive: true,
            navigation_write: true,
        },
        navigation: NavigationView {
            location: NavigationLocation {
                kind: "project_overview".to_owned(),
                conversation_id: None,
            },
            href: format!("/projects/{project_id}"),
            context_drawer_open: false,
            filters: serde_json::json!({}),
            pinned: false,
            last_opened_at: None,
            version: 1,
        },
        attention_count: 0,
    };
    insert_audit_event(
        &mut transaction,
        &headers,
        &principal,
        Some(project_id),
        "project.created",
        "project",
        project_id,
        serde_json::json!({
            "organization_id": request.organization_id,
            "project_key": project.key,
            "template": applied_template,
        }),
    )
    .await?;
    let response_body = serde_json::to_value(&project)
        .map_err(|_| AppError::Internal("serialize Project response"))?;
    sqlx::query(
        r#"
        UPDATE idempotency_keys
        SET response_status = 201, response_body = $4
        WHERE actor_id = $1 AND scope = $2 AND key = $3
        "#,
    )
    .bind(principal.actor_id)
    .bind(PROJECT_CREATE_SCOPE)
    .bind(&idempotency_key)
    .bind(response_body)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    project_response(StatusCode::CREATED, project)
}

async fn get_project(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ProjectView>, AppError> {
    Ok(Json(fetch_project(&state, &principal, project_id).await?))
}

async fn archive_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ArchiveProjectRequest>,
) -> Result<Json<ProjectView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let project = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_project_action(&principal, &project, Action::ProjectManage)?;
    if project.archived_at.is_some() {
        transaction.commit().await?;
        return Ok(Json(fetch_project(&state, &principal, project_id).await?));
    }
    if project.version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "project",
            current_version: project.version,
        });
    }
    let has_active_run = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agent_runs ar
            JOIN conversations c ON c.id = ar.conversation_id
            WHERE c.project_id = $1
              AND ar.state IN (
                  'created', 'queued', 'starting', 'working', 'blocked', 'recovering'
              )
        )
        "#,
    )
    .bind(project_id)
    .fetch_one(&mut *transaction)
    .await?;
    if has_active_run {
        return Err(AppError::ProjectHasActiveRuns);
    }
    sqlx::query(
        r#"
        UPDATE projects
        SET archived_at = now(), archived_by_actor_id = $2,
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .execute(&mut *transaction)
    .await?;
    insert_audit_event(
        &mut transaction,
        &headers,
        &principal,
        Some(project_id),
        "project.archived",
        "project",
        project_id,
        serde_json::json!({"previous_version": project.version}),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(fetch_project(&state, &principal, project_id).await?))
}

async fn get_navigation(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<NavigationView>, AppError> {
    let project = fetch_project_row(&state, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_project_action(&principal, &project, Action::NavigationRead)?;
    Ok(Json(navigation_view(&project)?))
}

async fn update_navigation(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<UpdateNavigationRequest>,
) -> Result<Json<NavigationView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let project = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_project_action(&principal, &project, Action::NavigationWrite)?;
    if project.archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }

    let current = sqlx::query_as::<_, NavigationStateRow>(
        r#"
        SELECT
            context_drawer_open,
            filters,
            pinned_at IS NOT NULL AS pinned,
            version
        FROM project_navigation_states
        WHERE project_id = $1 AND actor_id = $2
        FOR UPDATE
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let current_version = current.as_ref().map_or(0, |row| row.version);
    if current_version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "project_navigation",
            current_version,
        });
    }
    validate_navigation_location(&mut transaction, project_id, &request.location).await?;
    let filters = request.filters.unwrap_or_else(|| {
        current
            .as_ref()
            .map(|row| row.filters.clone())
            .unwrap_or_else(|| serde_json::json!({}))
    });
    if !filters.is_object()
        || serde_json::to_vec(&filters)
            .map_err(|_| AppError::Validation("navigation filters"))?
            .len()
            > MAX_FILTER_BYTES
    {
        return Err(AppError::Validation("navigation filters"));
    }
    let pinned = request
        .pinned
        .unwrap_or_else(|| current.as_ref().is_some_and(|row| row.pinned));
    let context_drawer_open = request
        .context_drawer_open
        .unwrap_or_else(|| current.as_ref().is_some_and(|row| row.context_drawer_open));
    let row = sqlx::query_as::<_, ProjectRow>(
        r#"
        INSERT INTO project_navigation_states (
            project_id, actor_id, location_kind, conversation_id,
            context_drawer_open, filters, pinned_at, last_opened_at, version
        )
        VALUES (
            $1, $2, $3, $4, $5, $6,
            CASE WHEN $7 THEN now() ELSE NULL END,
            now(), 1
        )
        ON CONFLICT (project_id, actor_id) DO UPDATE
        SET location_kind = EXCLUDED.location_kind,
            conversation_id = EXCLUDED.conversation_id,
            context_drawer_open = EXCLUDED.context_drawer_open,
            filters = EXCLUDED.filters,
            pinned_at = CASE
                WHEN $7 AND project_navigation_states.pinned_at IS NULL THEN now()
                WHEN $7 THEN project_navigation_states.pinned_at
                ELSE NULL
            END,
            last_opened_at = now(),
            version = project_navigation_states.version + 1,
            updated_at = now()
        WHERE project_navigation_states.version = $8
        RETURNING
            $1 AS id,
            (SELECT organization_id FROM projects WHERE id = $1) AS organization_id,
            (SELECT project_key FROM projects WHERE id = $1) AS project_key,
            (SELECT name FROM projects WHERE id = $1) AS name,
            (SELECT role FROM project_memberships WHERE project_id = $1 AND actor_id = $2) AS role,
            (SELECT version FROM projects WHERE id = $1) AS version,
            (SELECT archived_at::text FROM projects WHERE id = $1) AS archived_at,
            location_kind,
            conversation_id,
            context_drawer_open,
            filters,
            pinned_at::text AS pinned_at,
            last_opened_at::text AS last_opened_at,
            version AS navigation_version
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .bind(&request.location.kind)
    .bind(request.location.conversation_id)
    .bind(context_drawer_open)
    .bind(filters)
    .bind(pinned)
    .bind(request.expected_version)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::VersionConflict {
        resource: "project_navigation",
        current_version,
    })?;
    transaction.commit().await?;
    Ok(Json(navigation_view(&row)?))
}

async fn switch_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<SwitchProjectRequest>,
) -> Result<Json<SwitchProjectResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let project = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_project_action(&principal, &project, Action::NavigationWrite)?;
    if project.archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let current_version = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT version
        FROM project_navigation_states
        WHERE project_id = $1 AND actor_id = $2
        FOR UPDATE
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .unwrap_or(0);
    if current_version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "project_navigation",
            current_version,
        });
    }
    sqlx::query(
        r#"
        INSERT INTO project_navigation_states (
            project_id, actor_id, location_kind,
            context_drawer_open, filters, last_opened_at, version
        )
        VALUES ($1, $2, 'project_overview', false, '{}'::jsonb, now(), 1)
        ON CONFLICT (project_id, actor_id) DO UPDATE
        SET last_opened_at = now(),
            version = project_navigation_states.version + 1,
            updated_at = now()
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let project = fetch_project(&state, &principal, project_id).await?;
    let navigation = project.navigation.clone();
    let href = navigation.href.clone();
    Ok(Json(SwitchProjectResponse {
        project,
        navigation,
        href,
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SwitchProjectResponse {
    project: ProjectView,
    navigation: NavigationView,
    href: String,
}

async fn fetch_project(
    state: &AppState,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<ProjectView, AppError> {
    let row = fetch_project_row(state, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_project_action(principal, &row, Action::ProjectView)?;
    project_view(row)
}

async fn fetch_project_row(
    state: &AppState,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<ProjectRow>, AppError> {
    Ok(sqlx::query_as::<_, ProjectRow>(
        r#"
        SELECT
            p.id,
            p.organization_id,
            p.project_key,
            p.name,
            pm.role,
            p.version,
            p.archived_at::text AS archived_at,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN 'project_overview'
                ELSE pns.location_kind
            END AS location_kind,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN NULL
                ELSE pns.conversation_id
            END AS conversation_id,
            pns.context_drawer_open,
            pns.filters,
            pns.pinned_at::text AS pinned_at,
            pns.last_opened_at::text AS last_opened_at,
            pns.version AS navigation_version
        FROM projects p
        JOIN project_memberships pm
          ON pm.project_id = p.id AND pm.actor_id = $2 AND pm.enabled
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = $2
         AND om.enabled
        LEFT JOIN project_navigation_states pns
          ON pns.project_id = p.id AND pns.actor_id = $2
        LEFT JOIN conversations navc
          ON navc.project_id = p.id
         AND navc.id = pns.conversation_id
         AND navc.kind = pns.location_kind
         AND pns.location_kind IN ('channel', 'thread')
         AND navc.archived_at IS NULL
         AND NOT EXISTS (
             SELECT 1
             FROM conversations nav_parent
             WHERE navc.kind = 'thread'
               AND nav_parent.project_id = navc.project_id
               AND nav_parent.id = navc.parent_channel_id
               AND nav_parent.archived_at IS NOT NULL
         )
        WHERE p.id = $1
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&state.pool)
    .await?)
}

async fn fetch_project_row_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<ProjectRow>, AppError> {
    Ok(sqlx::query_as::<_, ProjectRow>(
        r#"
        SELECT
            p.id,
            p.organization_id,
            p.project_key,
            p.name,
            pm.role,
            p.version,
            p.archived_at::text AS archived_at,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN 'project_overview'
                ELSE pns.location_kind
            END AS location_kind,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN NULL
                ELSE pns.conversation_id
            END AS conversation_id,
            pns.context_drawer_open,
            pns.filters,
            pns.pinned_at::text AS pinned_at,
            pns.last_opened_at::text AS last_opened_at,
            pns.version AS navigation_version
        FROM projects p
        JOIN project_memberships pm
          ON pm.project_id = p.id AND pm.actor_id = $2 AND pm.enabled
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = $2
         AND om.enabled
        LEFT JOIN project_navigation_states pns
          ON pns.project_id = p.id AND pns.actor_id = $2
        LEFT JOIN conversations navc
          ON navc.project_id = p.id
         AND navc.id = pns.conversation_id
         AND navc.kind = pns.location_kind
         AND pns.location_kind IN ('channel', 'thread')
         AND navc.archived_at IS NULL
        WHERE p.id = $1
        FOR SHARE OF p, pm, om
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn lock_project_membership(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<ProjectMembershipRow>, AppError> {
    Ok(sqlx::query_as::<_, ProjectMembershipRow>(
        r#"
        SELECT
            p.id,
            pm.role,
            p.version,
            p.archived_at::text AS archived_at
        FROM projects p
        JOIN project_memberships pm
          ON pm.project_id = p.id AND pm.actor_id = $2 AND pm.enabled
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = $2
         AND om.enabled
        WHERE p.id = $1
        FOR UPDATE OF p, pm
        FOR SHARE OF om
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

fn project_view(row: ProjectRow) -> Result<ProjectView, AppError> {
    let role = membership_role(&row.role)?;
    let scope = ProjectScope::project(row.id);
    let subject = subject(Uuid::nil(), ActorKind::Human, row.id, role);
    let view = authorize(&subject, Action::ProjectView, Resource::Project(scope)).is_allowed();
    let manage = authorize(&subject, Action::ProjectManage, Resource::Project(scope)).is_allowed();
    let navigation_write =
        authorize(&subject, Action::NavigationWrite, Resource::Project(scope)).is_allowed();
    let navigation = navigation_view(&row)?;
    Ok(ProjectView {
        id: row.id,
        organization_id: row.organization_id,
        key: row.project_key,
        name: row.name,
        archived_at: row.archived_at,
        role: row.role,
        version: row.version,
        capabilities: ProjectCapabilities {
            view,
            manage,
            archive: manage,
            navigation_write,
        },
        navigation,
        attention_count: 0,
    })
}

fn navigation_view(row: &ProjectRow) -> Result<NavigationView, AppError> {
    let location = NavigationLocation {
        kind: row
            .location_kind
            .clone()
            .unwrap_or_else(|| "project_overview".to_owned()),
        conversation_id: row.conversation_id,
    };
    let href = navigation_href(row.id, &location)?;
    Ok(NavigationView {
        location,
        href,
        context_drawer_open: row.context_drawer_open.unwrap_or(false),
        filters: row.filters.clone().unwrap_or_else(|| serde_json::json!({})),
        pinned: row.pinned_at.is_some(),
        last_opened_at: row.last_opened_at.clone(),
        version: row.navigation_version.unwrap_or(0),
    })
}

fn navigation_href(project_id: Uuid, location: &NavigationLocation) -> Result<String, AppError> {
    match (location.kind.as_str(), location.conversation_id) {
        ("project_overview", None) => Ok(format!("/projects/{project_id}")),
        ("work_items", None) => Ok(format!("/projects/{project_id}/work-items")),
        ("settings", None) => Ok(format!("/projects/{project_id}/settings")),
        ("thread", Some(conversation_id)) => {
            Ok(format!("/projects/{project_id}/threads/{conversation_id}"))
        }
        ("channel", Some(conversation_id)) => {
            Ok(format!("/projects/{project_id}/channels/{conversation_id}"))
        }
        _ => Err(AppError::Internal("stored Project navigation location")),
    }
}

async fn validate_navigation_location(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    location: &NavigationLocation,
) -> Result<(), AppError> {
    match (location.kind.as_str(), location.conversation_id) {
        ("project_overview" | "work_items" | "settings", None) => Ok(()),
        (kind @ ("channel" | "thread"), Some(conversation_id)) => {
            let conversation = sqlx::query_as::<_, (String, Option<Uuid>)>(
                r#"
                SELECT kind, parent_channel_id
                FROM conversations
                WHERE id = $1 AND project_id = $2
                  AND kind = $3 AND archived_at IS NULL
                FOR SHARE
                "#,
            )
            .bind(conversation_id)
            .bind(project_id)
            .bind(kind)
            .fetch_optional(&mut **transaction)
            .await?;
            let Some((_, parent_channel_id)) = conversation else {
                return Err(AppError::NotFound("conversation"));
            };
            if let Some(parent_channel_id) = parent_channel_id {
                let parent_active = sqlx::query_scalar::<_, bool>(
                    r#"
                    SELECT archived_at IS NULL
                    FROM conversations
                    WHERE id = $1 AND project_id = $2 AND kind = 'channel'
                    FOR SHARE
                    "#,
                )
                .bind(parent_channel_id)
                .bind(project_id)
                .fetch_optional(&mut **transaction)
                .await?
                .unwrap_or(false);
                if !parent_active {
                    return Err(AppError::NotFound("conversation"));
                }
            }
            Ok(())
        }
        _ => Err(AppError::Validation("navigation location")),
    }
}

fn authorize_organization_create(
    principal: &AuthenticatedActor,
    organization_id: Uuid,
    role: &str,
) -> Result<(), AppError> {
    let role = membership_role(role)?;
    let subject = Subject::new(
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
        true,
        Some(Membership::organization(organization_id, role, true)),
        None,
    );
    allowed(authorize(
        &subject,
        Action::OrganizationProjectCreate,
        Resource::Organization { organization_id },
    ))
}

fn authorize_project_action(
    principal: &AuthenticatedActor,
    project: &impl ProjectMembership,
    action: Action,
) -> Result<(), AppError> {
    let role = membership_role(project.role())?;
    let subject = subject(
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
        project.project_id(),
        role,
    );
    allowed(authorize(
        &subject,
        action,
        Resource::Project(ProjectScope::project(project.project_id())),
    ))
}

trait ProjectMembership {
    fn project_id(&self) -> Uuid;
    fn role(&self) -> &str;
}

impl ProjectMembership for ProjectRow {
    fn project_id(&self) -> Uuid {
        self.id
    }

    fn role(&self) -> &str {
        &self.role
    }
}

impl ProjectMembership for ProjectMembershipRow {
    fn project_id(&self) -> Uuid {
        self.id
    }

    fn role(&self) -> &str {
        &self.role
    }
}

fn subject(
    actor_id: Uuid,
    actor_kind: ActorKind,
    project_id: Uuid,
    role: MembershipRole,
) -> Subject {
    Subject::new(
        actor_id,
        actor_kind,
        true,
        Some(Membership::project(project_id, role, true)),
        None,
    )
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

fn allowed(decision: Decision) -> Result<(), AppError> {
    if decision.is_allowed() {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn normalize_project_key(value: &str) -> Result<String, AppError> {
    let key = value.trim().to_ascii_uppercase();
    let mut characters = key.chars();
    let Some(first) = characters.next() else {
        return Err(AppError::Validation("Project key"));
    };
    if !(2..=10).contains(&key.len())
        || !first.is_ascii_uppercase()
        || !characters.all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
    {
        return Err(AppError::Validation("Project key"));
    }
    Ok(key)
}

pub(crate) fn normalize_project_name(value: &str) -> Result<String, AppError> {
    let name = value.trim();
    if name.is_empty() || name.len() > MAX_PROJECT_NAME_BYTES || name.chars().any(char::is_control)
    {
        return Err(AppError::Validation("Project name"));
    }
    Ok(name.to_owned())
}

pub(crate) fn membership_handle(login_name: &str) -> String {
    let mut handle = login_name
        .chars()
        .filter_map(|character| {
            character
                .is_ascii_alphanumeric()
                .then_some(character.to_ascii_lowercase())
                .or_else(|| matches!(character, '_' | '-').then_some(character))
                .or_else(|| (character == '.').then_some('_'))
        })
        .take(32)
        .collect::<String>();
    if handle.len() < 2 {
        handle.push_str("_user");
    }
    handle
}

fn database_constraint(error: &sqlx::Error) -> Option<&str> {
    match error {
        sqlx::Error::Database(database) => database.constraint(),
        _ => None,
    }
}

fn idempotency_key(headers: &HeaderMap) -> Result<String, AppError> {
    let value = headers
        .get("idempotency-key")
        .and_then(|header| header.to_str().ok())
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= MAX_IDEMPOTENCY_KEY_BYTES
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                })
        })
        .ok_or(AppError::Validation("Idempotency-Key"))?;
    Ok(value.to_owned())
}

fn project_response(status: StatusCode, project: ProjectView) -> Result<Response, AppError> {
    let location = HeaderValue::from_str(&format!("/api/v1/projects/{}", project.id))
        .map_err(|_| AppError::Internal("construct Project Location header"))?;
    Ok((status, [(LOCATION, location)], Json(project)).into_response())
}

#[allow(clippy::too_many_arguments)]
async fn insert_audit_event(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Option<Uuid>,
    action: &'static str,
    target_kind: &'static str,
    target_id: Uuid,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    let request_id = headers
        .get(crate::REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128);
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, metadata
        )
        VALUES (
            $1, $2, $3, $4, 'password_session',
            $5, $6, $7, 'succeeded', $8, $9
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .bind(request_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        NavigationLocation, membership_handle, navigation_href, normalize_project_key,
        normalize_project_name,
    };
    use uuid::Uuid;

    #[test]
    fn project_keys_are_normalized_and_bounded() {
        assert_eq!(normalize_project_key(" tri ").expect("key"), "TRI");
        assert!(normalize_project_key("1TRI").is_err());
        assert!(normalize_project_key("A").is_err());
        assert!(normalize_project_key("TOO-LONG-KEY").is_err());
    }

    #[test]
    fn project_names_reject_empty_control_or_oversized_input() {
        assert_eq!(
            normalize_project_name(" Agent Workspace ").expect("name"),
            "Agent Workspace"
        );
        assert!(normalize_project_name("").is_err());
        assert!(normalize_project_name("bad\nname").is_err());
        assert!(normalize_project_name(&"x".repeat(121)).is_err());
    }

    #[test]
    fn navigation_hrefs_are_generated_from_structured_locations() {
        let project_id = Uuid::now_v7();
        let conversation_id = Uuid::now_v7();
        assert_eq!(
            navigation_href(
                project_id,
                &NavigationLocation {
                    kind: "thread".to_owned(),
                    conversation_id: Some(conversation_id),
                },
            )
            .expect("thread href"),
            format!("/projects/{project_id}/threads/{conversation_id}")
        );
        assert!(
            navigation_href(
                project_id,
                &NavigationLocation {
                    kind: "thread".to_owned(),
                    conversation_id: None,
                },
            )
            .is_err()
        );
        assert_eq!(
            navigation_href(
                project_id,
                &NavigationLocation {
                    kind: "channel".to_owned(),
                    conversation_id: Some(conversation_id),
                },
            )
            .expect("channel href"),
            format!("/projects/{project_id}/channels/{conversation_id}")
        );
        assert_eq!(
            navigation_href(
                project_id,
                &NavigationLocation {
                    kind: "work_items".to_owned(),
                    conversation_id: None,
                },
            )
            .expect("Work Items href"),
            format!("/projects/{project_id}/work-items")
        );
    }

    #[test]
    fn membership_handles_are_safe_and_bounded() {
        assert_eq!(membership_handle("Mischa.Nelson"), "mischa_nelson");
        assert!(membership_handle(&"a".repeat(64)).len() <= 32);
    }
}

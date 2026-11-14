use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header::LOCATION},
    response::{IntoResponse, Response},
    routing::{get, post, put},
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
        Action, ActorKind, Decision, Membership, ProjectScope, Resource, RoleGrant, Subject,
        authorize,
    },
};

const MAX_PROJECT_NAME_BYTES: usize = 120;
const MAX_SEARCH_BYTES: usize = 120;
const MAX_FILTER_BYTES: usize = 50_000;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 200;
/// Most projects one rail order places.
const MAX_RAIL_PROJECTS: usize = 200;
const PROJECT_CREATE_SCOPE: &str = "projects.create";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/projects", get(list_projects).post(create_project))
        .route("/api/v1/projects/rail-order", put(set_rail_order))
        .route(
            "/api/v1/projects/{id}",
            get(get_project).patch(update_project),
        )
        .route("/api/v1/projects/{id}/archive", post(archive_project))
        .route("/api/v1/projects/{id}/restore", post(restore_project))
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
    role_permissions: Option<Vec<String>>,
    version: i64,
    archived_at: Option<String>,
    location_kind: Option<String>,
    conversation_id: Option<Uuid>,
    page_id: Option<Uuid>,
    context_drawer_open: Option<bool>,
    filters: Option<serde_json::Value>,
    pinned_at: Option<String>,
    rail_position: Option<i32>,
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
    /// Unread notifications in this project's inbox for the caller.
    attention_count: u32,
    /// AWD-75: the modules off for the caller here (the workspace's switches
    /// and their role), so the rail and the command menu leave out what
    /// they'd lead to without a request per project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    modules_off: Vec<String>,
    /// AWD-96: this is the workspace's own space for pages, not a project.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    workspace_space: bool,
}

/// AWD-75: a module set's off modules, by id.
fn off_ids(modules: crate::modules::ModuleSet) -> Vec<String> {
    modules.off().map(|module| module.id.clone()).collect()
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
    /// Where you dragged this pinned project in your rail; unplaced pins follow.
    rail_position: Option<i32>,
    last_opened_at: Option<String>,
    version: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NavigationLocation {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    page_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectListQuery {
    q: Option<String>,
    limit: Option<u16>,
    /// Archived projects instead of active ones, most recently archived first.
    archived: Option<bool>,
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
struct UpdateProjectRequest {
    expected_version: i64,
    name: String,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RailOrderRequest {
    /// Your pinned projects, top to bottom.
    project_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RailOrderResponse {
    /// The listed projects that were placed: those you can still open.
    project_ids: Vec<Uuid>,
}

#[derive(Debug, FromRow)]
struct OrganizationMembershipRow {
    role: String,
    role_permissions: Option<Vec<String>>,
}

#[derive(Debug, FromRow)]
struct ProjectMembershipRow {
    id: Uuid,
    role: String,
    role_permissions: Option<Vec<String>>,
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
            workspace_role_permissions(pm.role_id) AS role_permissions,
            p.version,
            p.archived_at::text AS archived_at,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN 'project_overview'
                -- A page that was archived since falls back to the pages list.
                WHEN pns.location_kind = 'page' AND navp.id IS NULL THEN 'pages'
                ELSE pns.location_kind
            END AS location_kind,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN NULL
                ELSE pns.conversation_id
            END AS conversation_id,
            CASE WHEN p.archived_at IS NULL THEN navp.id END AS page_id,
            pns.context_drawer_open,
            pns.filters,
            pns.pinned_at::text AS pinned_at,
            pns.rail_position,
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
        LEFT JOIN project_pages navp
          ON navp.project_id = p.id
         AND navp.id = pns.page_id
         AND pns.location_kind = 'page'
         AND navp.archived_at IS NULL
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
          AND (p.archived_at IS NOT NULL) = $5
          AND p.kind = 'standard'
          AND (
              $2::text IS NULL
              OR p.name ILIKE $2
              OR p.project_key ILIKE $2
          )
        ORDER BY
            -- An exact key comes first, so `/KEY-12` finds its Project before the limit.
            (upper(p.project_key) = upper($4)) IS TRUE DESC,
            p.archived_at DESC NULLS LAST,
            (pns.pinned_at IS NOT NULL) DESC,
            pns.rail_position ASC NULLS LAST,
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
    .bind(query.archived.unwrap_or(false))
    .fetch_all(&state.pool)
    .await?;
    let attention = crate::notifications::attention_counts(&state.pool, principal.actor_id).await?;
    let mut projects = rows
        .into_iter()
        .map(|row| {
            let mut view = project_view(row)?;
            view.attention_count = attention.get(&view.id).copied().unwrap_or(0);
            Ok(view)
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    // AWD-75: each project says which modules are off for the caller there.
    let ids = projects.iter().map(|view| view.id).collect::<Vec<_>>();
    let modules = {
        let mut connection = state.pool.acquire().await?;
        crate::modules::state::ProjectModules::load(&mut connection, principal.actor_id, &ids)
            .await?
    };
    for view in &mut projects {
        view.modules_off = off_ids(modules.of(view.id));
    }
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
        SELECT role, workspace_role_permissions(role_id) AS role_permissions
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
    authorize_organization_create(
        &principal,
        request.organization_id,
        &membership.role,
        membership.role_permissions.as_deref(),
    )?;

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

    let project_id = insert_new_project(
        &mut transaction,
        request.organization_id,
        &key,
        &name,
        ProjectOwner {
            actor_id: principal.actor_id,
            display_name: &principal.display_name,
            login_name: &principal.login_name,
        },
    )
    .await?;
    let applied_template = match request.template_id {
        Some(template_id) => Some(
            crate::work_items::project_templates::apply_to_new_project(
                &mut transaction,
                crate::work_items::project_templates::creator_actor(&principal, project_id)?,
                request.organization_id,
                template_id,
            )
            .await?,
        ),
        None => None,
    };

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
                page_id: None,
            },
            href: format!("/projects/{project_id}"),
            context_drawer_open: false,
            filters: serde_json::json!({}),
            pinned: false,
            rail_position: None,
            last_opened_at: None,
            version: 1,
        },
        attention_count: 0,
        modules_off: Vec::new(),
        workspace_space: false,
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

/// Who a new Project is created for: they become its owner.
pub(crate) struct ProjectOwner<'a> {
    pub actor_id: Uuid,
    pub display_name: &'a str,
    pub login_name: &'a str,
}

/// The rows every new Project starts with: the Project, its owner, its task
/// counter, the default workflow, empty instructions and the owner's
/// navigation state. The web app and the MCP project_create tool both create
/// Projects through this, after checking the creator may.
pub(crate) async fn insert_new_project(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    key: &str,
    name: &str,
    owner: ProjectOwner<'_>,
) -> Result<Uuid, AppError> {
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
    .bind(organization_id)
    .bind(key)
    .bind(name)
    .execute(&mut **transaction)
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
    .bind(owner.actor_id)
    .bind(owner.display_name)
    .bind(membership_handle(owner.login_name))
    .execute(&mut **transaction)
    .await?;
    sqlx::query("INSERT INTO project_counters (project_id, next_work_item_number) VALUES ($1, 1)")
        .bind(project_id)
        .execute(&mut **transaction)
        .await?;
    crate::work_items::ensure_default_workflow(transaction, project_id).await?;
    sqlx::query(
        r#"
        INSERT INTO project_instructions (project_id, content)
        VALUES ($1, '')
        "#,
    )
    .bind(project_id)
    .execute(&mut **transaction)
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
    .bind(owner.actor_id)
    .execute(&mut **transaction)
    .await?;
    Ok(project_id)
}

async fn get_project(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ProjectView>, AppError> {
    Ok(Json(fetch_project(&state, &principal, project_id).await?))
}

/// Renames a project from its settings, as `project_update` does over MCP:
/// whoever may manage it, against the version they read. The key stays, and
/// the Messages space keeps its name.
async fn update_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<UpdateProjectRequest>,
) -> Result<Json<ProjectView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let name = normalize_project_name(&request.name)?;
    let mut transaction = state.pool.begin().await?;
    let project = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_project_action(&principal, &project, Action::ProjectManage)?;
    if project.archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    if project.version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "project",
            current_version: project.version,
        });
    }
    let (kind, current_name) =
        sqlx::query_as::<_, (String, String)>("SELECT kind, name FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut *transaction)
            .await?;
    if kind == "direct" {
        return Err(AppError::Forbidden);
    }
    if current_name != name {
        sqlx::query(
            "UPDATE projects SET name = $2, version = version + 1, updated_at = now() WHERE id = $1",
        )
        .bind(project_id)
        .bind(&name)
        .execute(&mut *transaction)
        .await?;
        insert_audit_event(
            &mut transaction,
            &headers,
            &principal,
            Some(project_id),
            "project.renamed",
            "project",
            project_id,
            serde_json::json!({
                "previous_name": current_name,
                "previous_version": project.version,
            }),
        )
        .await?;
    }
    transaction.commit().await?;
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
    let has_active_run =
        crate::runs::store::active_under_space(&mut *transaction, project_id).await?;
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

/// Brings an archived project back, for whoever may manage it, against the
/// version they read. Its channels, tasks and settings were kept as they were.
async fn restore_project(
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
    if project.archived_at.is_none() {
        transaction.commit().await?;
        return Ok(Json(fetch_project(&state, &principal, project_id).await?));
    }
    if project.version != request.expected_version {
        return Err(AppError::VersionConflict {
            resource: "project",
            current_version: project.version,
        });
    }
    let restored = sqlx::query(
        r#"
        UPDATE projects
        SET archived_at = NULL, archived_by_actor_id = NULL,
            version = version + 1, updated_at = now()
        WHERE id = $1 AND kind = 'standard'
        "#,
    )
    .bind(project_id)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if restored == 0 {
        return Err(AppError::Forbidden);
    }
    insert_audit_event(
        &mut transaction,
        &headers,
        &principal,
        Some(project_id),
        "project.restored",
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
            context_drawer_open, filters, pinned_at, last_opened_at, version,
            page_id
        )
        VALUES (
            $1, $2, $3, $4, $5, $6,
            CASE WHEN $7 THEN now() ELSE NULL END,
            now(), 1, $9
        )
        ON CONFLICT (project_id, actor_id) DO UPDATE
        SET location_kind = EXCLUDED.location_kind,
            conversation_id = EXCLUDED.conversation_id,
            page_id = EXCLUDED.page_id,
            context_drawer_open = EXCLUDED.context_drawer_open,
            filters = EXCLUDED.filters,
            pinned_at = CASE
                WHEN $7 AND project_navigation_states.pinned_at IS NULL THEN now()
                WHEN $7 THEN project_navigation_states.pinned_at
                ELSE NULL
            END,
            -- Pinned again later, a project goes to the end, not its old place.
            rail_position = CASE WHEN $7 THEN project_navigation_states.rail_position END,
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
            (SELECT workspace_role_permissions(role_id) FROM project_memberships
              WHERE project_id = $1 AND actor_id = $2) AS role_permissions,
            (SELECT version FROM projects WHERE id = $1) AS version,
            (SELECT archived_at::text FROM projects WHERE id = $1) AS archived_at,
            location_kind,
            conversation_id,
            page_id,
            context_drawer_open,
            filters,
            pinned_at::text AS pinned_at,
            rail_position,
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
    .bind(request.location.page_id)
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

/// Puts your rail in your own order, from a drag: each listed project is
/// pinned at its place, and your other pins keep their pin and follow, in the
/// order you pinned them. Only your own navigation changes. A listed project
/// you can't open any more (removed, archived) is left out, not refused, so
/// a rail drawn a moment ago still saves.
async fn set_rail_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(request): ApiJson<RailOrderRequest>,
) -> Result<Json<RailOrderResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut seen = std::collections::HashSet::new();
    if request.project_ids.len() > MAX_RAIL_PROJECTS
        || !request.project_ids.iter().all(|id| seen.insert(*id))
    {
        return Err(AppError::Validation("project_ids"));
    }
    let mut transaction = state.pool.begin().await?;
    let memberships = sqlx::query_as::<_, ProjectMembershipRow>(
        r#"
        SELECT
            p.id,
            pm.role,
            workspace_role_permissions(pm.role_id) AS role_permissions,
            p.version,
            p.archived_at::text AS archived_at
        FROM projects p
        JOIN project_memberships pm
          ON pm.project_id = p.id AND pm.actor_id = $1 AND pm.enabled
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = $1
         AND om.enabled
        WHERE p.id = ANY($2)
          AND p.kind = 'standard'
          AND p.archived_at IS NULL
        ORDER BY p.id
        FOR SHARE OF pm, om
        "#,
    )
    .bind(principal.actor_id)
    .bind(&request.project_ids)
    .fetch_all(&mut *transaction)
    .await?;
    let allowed = memberships
        .iter()
        .filter(|membership| {
            authorize_project_action(&principal, *membership, Action::NavigationWrite).is_ok()
        })
        .map(|membership| membership.id)
        .collect::<std::collections::HashSet<_>>();
    let placed = request
        .project_ids
        .into_iter()
        .filter(|id| allowed.contains(id))
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        UPDATE project_navigation_states
        SET rail_position = NULL, updated_at = now()
        WHERE actor_id = $1
          AND rail_position IS NOT NULL
          AND project_id <> ALL($2)
        "#,
    )
    .bind(principal.actor_id)
    .bind(&placed)
    .execute(&mut *transaction)
    .await?;
    // The rest of a project's navigation and its version stay as they are:
    // the order is its own setting, so it never conflicts with a place being
    // remembered at the same moment.
    sqlx::query(
        r#"
        INSERT INTO project_navigation_states (
            project_id, actor_id, location_kind, context_drawer_open, filters,
            pinned_at, rail_position, last_opened_at, version
        )
        SELECT placed.project_id, $1, 'project_overview', false, '{}'::jsonb,
               now(), placed.position::integer - 1, now(), 1
        FROM unnest($2::uuid[]) WITH ORDINALITY AS placed(project_id, position)
        ON CONFLICT (project_id, actor_id) DO UPDATE
        SET pinned_at = COALESCE(project_navigation_states.pinned_at, now()),
            rail_position = EXCLUDED.rail_position,
            updated_at = now()
        "#,
    )
    .bind(principal.actor_id)
    .bind(&placed)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Json(RailOrderResponse {
        project_ids: placed,
    }))
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
    let mut view = project_view(row)?;
    view.attention_count = crate::notifications::attention_counts(&state.pool, principal.actor_id)
        .await?
        .get(&project_id)
        .copied()
        .unwrap_or(0);
    // AWD-75: as the list says it, so a project opened or switched to keeps
    // what's off for the caller there (its rail tile too, with Projects off).
    let mut connection = state.pool.acquire().await?;
    view.modules_off =
        crate::modules::state::for_project(&mut connection, project_id, principal.actor_id)
            .await?
            .map(off_ids)
            .unwrap_or_default();
    view.workspace_space = crate::workspace_space::find(&mut connection, view.organization_id)
        .await?
        == Some(project_id);
    Ok(view)
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
            workspace_role_permissions(pm.role_id) AS role_permissions,
            p.version,
            p.archived_at::text AS archived_at,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN 'project_overview'
                -- A page that was archived since falls back to the pages list.
                WHEN pns.location_kind = 'page' AND navp.id IS NULL THEN 'pages'
                ELSE pns.location_kind
            END AS location_kind,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN NULL
                ELSE pns.conversation_id
            END AS conversation_id,
            CASE WHEN p.archived_at IS NULL THEN navp.id END AS page_id,
            pns.context_drawer_open,
            pns.filters,
            pns.pinned_at::text AS pinned_at,
            pns.rail_position,
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
        LEFT JOIN project_pages navp
          ON navp.project_id = p.id
         AND navp.id = pns.page_id
         AND pns.location_kind = 'page'
         AND navp.archived_at IS NULL
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
            workspace_role_permissions(pm.role_id) AS role_permissions,
            p.version,
            p.archived_at::text AS archived_at,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN 'project_overview'
                -- A page that was archived since falls back to the pages list.
                WHEN pns.location_kind = 'page' AND navp.id IS NULL THEN 'pages'
                ELSE pns.location_kind
            END AS location_kind,
            CASE
                WHEN p.archived_at IS NOT NULL
                  OR (pns.location_kind IN ('channel', 'thread') AND navc.id IS NULL)
                THEN NULL
                ELSE pns.conversation_id
            END AS conversation_id,
            CASE WHEN p.archived_at IS NULL THEN navp.id END AS page_id,
            pns.context_drawer_open,
            pns.filters,
            pns.pinned_at::text AS pinned_at,
            pns.rail_position,
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
        LEFT JOIN project_pages navp
          ON navp.project_id = p.id
         AND navp.id = pns.page_id
         AND pns.location_kind = 'page'
         AND navp.archived_at IS NULL
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
            workspace_role_permissions(pm.role_id) AS role_permissions,
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
    let role = RoleGrant::stored(&row.role, row.role_permissions.as_deref())?;
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
        modules_off: Vec::new(),
        workspace_space: false,
    })
}

fn navigation_view(row: &ProjectRow) -> Result<NavigationView, AppError> {
    let location = NavigationLocation {
        kind: row
            .location_kind
            .clone()
            .unwrap_or_else(|| "project_overview".to_owned()),
        conversation_id: row.conversation_id,
        page_id: row.page_id,
    };
    let href = navigation_href(row.id, &location)?;
    Ok(NavigationView {
        location,
        href,
        context_drawer_open: row.context_drawer_open.unwrap_or(false),
        filters: row.filters.clone().unwrap_or_else(|| serde_json::json!({})),
        pinned: row.pinned_at.is_some(),
        rail_position: row.rail_position,
        last_opened_at: row.last_opened_at.clone(),
        version: row.navigation_version.unwrap_or(0),
    })
}

/// The address of a place in a project that doesn't name a conversation or a page.
fn plain_location_path(kind: &str) -> Option<&'static str> {
    Some(match kind {
        // Older clients saved settings as the last place; opening the Project
        // from there should lead out of settings, so it lands on the overview.
        "project_overview" | "settings" => "",
        "work_items" => "/work-items",
        "milestones" => "/milestones",
        "timeline" => "/timeline",
        "planning" => "/next",
        "pages" => "/pages",
        "decisions" => "/decisions",
        "runs" => "/launches",
        _ => return None,
    })
}

fn navigation_href(project_id: Uuid, location: &NavigationLocation) -> Result<String, AppError> {
    match (
        location.kind.as_str(),
        location.conversation_id,
        location.page_id,
    ) {
        ("thread", Some(conversation_id), None) => {
            Ok(format!("/projects/{project_id}/threads/{conversation_id}"))
        }
        ("channel", Some(conversation_id), None) => {
            Ok(format!("/projects/{project_id}/channels/{conversation_id}"))
        }
        ("page", None, Some(page_id)) => Ok(format!("/projects/{project_id}/pages/{page_id}")),
        (kind, None, None) => plain_location_path(kind)
            .map(|path| format!("/projects/{project_id}{path}"))
            .ok_or(AppError::Internal("stored Project navigation location")),
        _ => Err(AppError::Internal("stored Project navigation location")),
    }
}

async fn validate_navigation_location(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    location: &NavigationLocation,
) -> Result<(), AppError> {
    match (
        location.kind.as_str(),
        location.conversation_id,
        location.page_id,
    ) {
        ("page", None, Some(page_id)) => {
            let active = sqlx::query_scalar::<_, bool>(
                r#"
                SELECT archived_at IS NULL
                FROM project_pages
                WHERE id = $1 AND project_id = $2
                FOR SHARE
                "#,
            )
            .bind(page_id)
            .bind(project_id)
            .fetch_optional(&mut **transaction)
            .await?
            .unwrap_or(false);
            if active {
                Ok(())
            } else {
                Err(AppError::NotFound("page"))
            }
        }
        (kind, None, None) if plain_location_path(kind).is_some() => Ok(()),
        (kind @ ("channel" | "thread"), Some(conversation_id), None) => {
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
    role_permissions: Option<&[String]>,
) -> Result<(), AppError> {
    authorize_project_create(
        principal.actor_id,
        actor_kind(&principal.actor_kind)?,
        organization_id,
        role,
        role_permissions,
    )
}

/// Whether this actor, with this role in the workspace, may create Projects
/// there.
pub(crate) fn authorize_project_create(
    actor_id: Uuid,
    kind: ActorKind,
    organization_id: Uuid,
    role: &str,
    role_permissions: Option<&[String]>,
) -> Result<(), AppError> {
    let role = RoleGrant::stored(role, role_permissions)?;
    let subject = Subject::new(
        actor_id,
        kind,
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
    let role = RoleGrant::stored(project.role(), project.role_permissions())?;
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
    fn role_permissions(&self) -> Option<&[String]>;
}

impl ProjectMembership for ProjectRow {
    fn project_id(&self) -> Uuid {
        self.id
    }

    fn role(&self) -> &str {
        &self.role
    }

    fn role_permissions(&self) -> Option<&[String]> {
        self.role_permissions.as_deref()
    }
}

impl ProjectMembership for ProjectMembershipRow {
    fn project_id(&self) -> Uuid {
        self.id
    }

    fn role(&self) -> &str {
        &self.role
    }

    fn role_permissions(&self) -> Option<&[String]> {
        self.role_permissions.as_deref()
    }
}

fn subject(actor_id: Uuid, actor_kind: ActorKind, project_id: Uuid, role: RoleGrant) -> Subject {
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

fn allowed(decision: Decision) -> Result<(), AppError> {
    if decision.is_allowed() {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

pub(crate) fn normalize_project_key(value: &str) -> Result<String, AppError> {
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
                    page_id: None,
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
                    page_id: None,
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
                    page_id: None,
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
                    page_id: None,
                },
            )
            .expect("Work Items href"),
            format!("/projects/{project_id}/work-items")
        );
        let page_id = Uuid::now_v7();
        let place = |kind: &str, page_id: Option<Uuid>| {
            navigation_href(
                project_id,
                &NavigationLocation {
                    kind: kind.to_owned(),
                    conversation_id: None,
                    page_id,
                },
            )
        };
        assert_eq!(
            place("page", Some(page_id)).expect("page href"),
            format!("/projects/{project_id}/pages/{page_id}")
        );
        assert!(place("page", None).is_err());
        assert!(place("pages", Some(page_id)).is_err());
        assert_eq!(
            place("planning", None).expect("planning href"),
            format!("/projects/{project_id}/next")
        );
        assert_eq!(
            place("runs", None).expect("runs href"),
            format!("/projects/{project_id}/launches")
        );
        assert_eq!(
            place("settings", None).expect("settings href"),
            format!("/projects/{project_id}")
        );
        assert!(place("somewhere", None).is_err());
    }

    #[test]
    fn membership_handles_are_safe_and_bounded() {
        assert_eq!(membership_handle("Mischa.Nelson"), "mischa_nelson");
        assert!(membership_handle(&"a".repeat(64)).len() <= 32);
    }
}

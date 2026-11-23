use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    addresses::{Place, Target},
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    policy::{Action, RoleGrant},
    spaces::SpaceKind,
};

const MAX_SEARCH_BYTES: usize = 120;
const MAX_FILTER_BYTES: usize = 50_000;
/// Most projects one rail order places.
const MAX_RAIL_PROJECTS: usize = 200;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/projects", get(list_projects))
        .route("/api/v1/projects/rail-order", put(set_rail_order))
        .route("/api/v1/projects/{id}", get(get_project))
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
    icon: Option<String>,
    kind: String,
    role: String,
    role_permissions: Option<Vec<String>>,
    /// What the caller's groups are given here (K1.27).
    #[sqlx(default)]
    group_permissions: Option<Vec<String>>,
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
    /// An emoji shown beside the name (K4.7), when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    archived_at: Option<String>,
    role: String,
    /// What the caller's role lets them do here (K1.26), before any app's
    /// limit: the web asks this, never the role's name.
    #[serde(default)]
    permissions: Vec<String>,
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
    /// The kind of space (K4.10): `project`, or `workspace` for the
    /// workspace's own space, which is never listed as a project.
    #[serde(default = "project_kind")]
    kind: SpaceKind,
}

fn project_kind() -> SpaceKind {
    SpaceKind::Project
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
    /// Only this workspace's, so the limit counts its projects alone.
    workspace_id: Option<Uuid>,
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
struct ProjectMembershipRow {
    id: Uuid,
    role: String,
    role_permissions: Option<Vec<String>>,
    group_permissions: Option<Vec<String>>,
    archived_at: Option<String>,
}

#[derive(Debug, FromRow)]
struct NavigationStateRow {
    context_drawer_open: bool,
    filters: serde_json::Value,
    pinned: bool,
    version: i64,
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
            .check_expensive_read(principal.session_id)
            .await?;
    }
    let search_pattern = search.map(|value| format!("%{value}%"));
    let rows = sqlx::query_as::<_, ProjectRow>(
        r#"
        SELECT
            p.id,
            p.organization_id,
            p.project_key,
            p.icon,
            p.name,
            p.kind,
            pm.role,
            workspace_role_permissions(pm.role_id) AS role_permissions,
            space_group_permissions(pm.actor_id, pm.project_id) AS group_permissions,
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
          AND ($6::uuid IS NULL OR p.organization_id = $6)
          AND p.kind = 'standard'
          -- K4.2: the one readability rule (archived projects stay listed
          -- for their members, to be restored).
          AND (p.archived_at IS NOT NULL OR space_readable_by(p.id, $1))
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
    .bind(query.workspace_id)
    .fetch_all(crate::db::scoped(&state.pool))
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
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        crate::modules::state::ProjectModules::load(&mut connection, principal.actor_id, &ids)
            .await?
    };
    for view in &mut projects {
        view.modules_off = off_ids(modules.of(view.id));
    }
    Ok(Json(ProjectsResponse { projects }))
}

async fn get_project(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ProjectView>, AppError> {
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
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
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
            (SELECT kind FROM projects WHERE id = $1) AS kind,
            (SELECT role FROM project_memberships WHERE project_id = $1 AND actor_id = $2) AS role,
            (SELECT workspace_role_permissions(role_id) FROM project_memberships
              WHERE project_id = $1 AND actor_id = $2) AS role_permissions,
            space_group_permissions($2, $1) AS group_permissions,
            (SELECT version FROM projects WHERE id = $1) AS version,
            (SELECT archived_at::text FROM projects WHERE id = $1) AS archived_at,
            (SELECT icon FROM projects WHERE id = $1) AS icon,
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
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
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
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut seen = std::collections::HashSet::new();
    if request.project_ids.len() > MAX_RAIL_PROJECTS
        || !request.project_ids.iter().all(|id| seen.insert(*id))
    {
        return Err(AppError::Validation("project_ids"));
    }
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let memberships = sqlx::query_as::<_, ProjectMembershipRow>(
        r#"
        SELECT
            p.id,
            pm.role,
            workspace_role_permissions(pm.role_id) AS role_permissions,
            space_group_permissions(pm.actor_id, pm.project_id) AS group_permissions,
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
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    view.modules_off =
        crate::modules::state::for_project(&mut connection, project_id, principal.actor_id)
            .await?
            .map(off_ids)
            .unwrap_or_default();
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
            p.icon,
            p.name,
            p.kind,
            pm.role,
            workspace_role_permissions(pm.role_id) AS role_permissions,
            space_group_permissions(pm.actor_id, pm.project_id) AS group_permissions,
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
          -- K4.2: the one readability rule (an archived project stays
          -- readable to its members, to be restored).
          AND (p.archived_at IS NOT NULL OR space_readable_by(p.id, $2))
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(crate::db::scoped(&state.pool))
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
            space_group_permissions(pm.actor_id, pm.project_id) AS group_permissions,
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
    // What the role itself lets its holder do here, before any app's limit.
    let role = RoleGrant::stored(&row.role, row.role_permissions.as_deref())?.with_groups(
        crate::access::group_permissions(row.group_permissions.as_deref()),
    );
    let holds = |action: Action| {
        crate::access::membership_allows(Uuid::nil(), row.id, role, true, action.permission())
    };
    let view = holds(Action::ProjectView);
    let manage = holds(Action::ProjectManage);
    let navigation_write = holds(Action::NavigationWrite);
    let navigation = navigation_view(&row)?;
    Ok(ProjectView {
        id: row.id,
        organization_id: row.organization_id,
        key: row.project_key,
        name: row.name,
        icon: row.icon,
        archived_at: row.archived_at,
        role: row.role,
        permissions: role
            .permissions()
            .keys()
            .into_iter()
            .map(str::to_owned)
            .collect(),
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
        kind: SpaceKind::from_stored(&row.kind).unwrap_or(SpaceKind::Project),
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

/// A place in a project that doesn't name a conversation or a page.
fn plain_location_path(kind: &str) -> Option<Place> {
    Some(match kind {
        // Older clients saved settings as the last place; opening the Project
        // from there should lead out of settings, so it lands on the overview.
        "project_overview" | "settings" => Place::Home,
        "work_items" => Place::Tasks,
        "milestones" => Place::Milestones,
        "timeline" => Place::Timeline,
        "planning" => Place::Next,
        "pages" => Place::Pages,
        "decisions" => Place::Decisions,
        "runs" => Place::Runs,
        _ => return None,
    })
}

fn navigation_href(project_id: Uuid, location: &NavigationLocation) -> Result<String, AppError> {
    match (
        location.kind.as_str(),
        location.conversation_id,
        location.page_id,
    ) {
        ("thread", Some(conversation_id), None) => Ok(Place::Thread(conversation_id)),
        ("channel", Some(conversation_id), None) => Ok(Place::Channel(conversation_id)),
        ("page", None, Some(page_id)) => Ok(Place::Page(page_id)),
        (kind, None, None) => plain_location_path(kind)
            .ok_or(AppError::Internal("stored Project navigation location")),
        _ => Err(AppError::Internal("stored Project navigation location")),
    }
    .map(|place| Target::space(project_id, place).href())
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

/// Whether this actor, with this role in the workspace, may create Projects
/// there.
pub(crate) fn authorize_project_create(
    actor_id: Uuid,
    organization_id: Uuid,
    role: &str,
    role_permissions: Option<&[String]>,
) -> Result<(), AppError> {
    let role = RoleGrant::stored(role, role_permissions)?;
    if crate::access::workspace_membership_allows(
        actor_id,
        organization_id,
        role,
        true,
        Action::OrganizationProjectCreate.permission(),
    ) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn authorize_project_action(
    principal: &AuthenticatedActor,
    project: &impl ProjectMembership,
    action: Action,
) -> Result<(), AppError> {
    let role = RoleGrant::stored(project.role(), project.role_permissions())?.with_groups(
        crate::access::group_permissions(project.group_permissions()),
    );
    if crate::access::membership_allows(
        principal.actor_id,
        project.project_id(),
        role,
        true,
        action.permission(),
    ) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

trait ProjectMembership {
    fn project_id(&self) -> Uuid;
    fn role(&self) -> &str;
    fn role_permissions(&self) -> Option<&[String]>;
    fn group_permissions(&self) -> Option<&[String]>;
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

    fn group_permissions(&self) -> Option<&[String]> {
        self.group_permissions.as_deref()
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

    fn group_permissions(&self) -> Option<&[String]> {
        self.group_permissions.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::{NavigationLocation, navigation_href};
    use uuid::Uuid;

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
            format!("/w/-/s/{project_id}/threads/{conversation_id}")
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
            format!("/w/-/s/{project_id}/chat/{conversation_id}")
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
            format!("/w/-/s/{project_id}/tasks")
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
            format!("/w/-/s/{project_id}/pages/{page_id}")
        );
        assert!(place("page", None).is_err());
        assert!(place("pages", Some(page_id)).is_err());
        assert_eq!(
            place("planning", None).expect("planning href"),
            format!("/w/-/s/{project_id}/planning/next")
        );
        assert_eq!(
            place("runs", None).expect("runs href"),
            format!("/w/-/s/{project_id}/runs")
        );
        assert_eq!(
            place("settings", None).expect("settings href"),
            format!("/w/-/s/{project_id}")
        );
        assert!(place("somewhere", None).is_err());
    }
}

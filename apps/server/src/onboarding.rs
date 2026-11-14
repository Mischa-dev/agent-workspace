//! P5: the first-run path. A new workspace goes from naming itself to its
//! first launched run: create a project, connect GitHub and pick a
//! repository, connect a provider account, choose an environment and run
//! defaults, add tasks, optionally add an agent, then launch.
//!
//! AWD-76: it starts with picking a template (Everything first), which sets
//! the modules, roles, layout and a starter project, and then says who gets
//! what in plain words. Steps of modules the workspace has off don't show,
//! so a workspace without Code never sees "Connect GitHub".
//!
//! Each step is read from what exists, as the person sees it, so doing a
//! step anywhere in the app ticks it here. Only what can't be read is
//! stored: the project the path sets up, the steps a person confirmed
//! (naming the workspace) or skipped (adding an agent), and whether they
//! put the path away.

use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    launch,
};

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/organizations/{organization_id}/onboarding",
        get(read_http).put(update_http),
    )
}

/// The steps, in order.
pub const STEPS: [&str; 11] = [
    "template",
    "workspace",
    "roles",
    "project",
    "github",
    "repository",
    "account",
    "environment",
    "tasks",
    "agent",
    "launch",
];

/// Steps a person may skip.
const OPTIONAL: [&str; 1] = ["agent"];
/// Steps only a person's word can tick.
const CONFIRMED: [&str; 2] = ["workspace", "roles"];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub id: &'static str,
    pub done: bool,
    pub skipped: bool,
    pub optional: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Onboarding {
    pub organization_id: Uuid,
    pub organization_name: String,
    /// The project the path is setting up, while the person can open it.
    pub project_id: Option<Uuid>,
    pub project_name: Option<String>,
    pub steps: Vec<Step>,
    /// The first step neither done nor skipped.
    pub next_step: Option<&'static str>,
    pub complete: bool,
    pub dismissed: bool,
    /// Whether the person can rename the workspace (owners and admins).
    pub can_manage_workspace: bool,
}

#[derive(sqlx::FromRow)]
struct Progress {
    project_id: Option<Uuid>,
    confirmed_steps: Vec<String>,
    skipped_steps: Vec<String>,
    dismissed: bool,
}

/// The person's role in the workspace, while they are an enabled member.
async fn membership(
    pool: &PgPool,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<(String, String), AppError> {
    sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT organization.name, membership.role
        FROM organization_memberships membership
        JOIN organizations organization ON organization.id = membership.organization_id
        WHERE membership.organization_id = $1 AND membership.actor_id = $2 AND membership.enabled
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("organization"))
}

/// The project the path is about: the one it set up while it's still open
/// to the person, else the one they opened last in this workspace.
async fn chosen_project(
    pool: &PgPool,
    organization_id: Uuid,
    actor_id: Uuid,
    stored: Option<Uuid>,
) -> Result<Option<(Uuid, String)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT project.id, project.name
        FROM projects project
        LEFT JOIN project_navigation_states navigation
          ON navigation.project_id = project.id AND navigation.actor_id = $2
        WHERE project.organization_id = $1 AND project.kind = 'standard'
          AND project.archived_at IS NULL
          AND actor_can_access_project($2, NULL, project.id)
        ORDER BY project.id = $3 DESC, navigation.last_opened_at DESC NULLS LAST,
                 project.created_at, project.id
        LIMIT 1
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(stored)
    .fetch_optional(pool)
    .await?)
}

async fn exists(
    pool: &PgPool,
    query: &str,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar::<_, bool>(&format!("SELECT EXISTS ({query})"))
            .bind(project_id)
            .bind(actor_id)
            .fetch_one(pool)
            .await?,
    )
}

/// Whether each project step is done, for the person, in `project_id`.
async fn project_steps(
    state: &AppState,
    project_id: Uuid,
    actor_id: Uuid,
) -> Result<[(&'static str, bool); 7], AppError> {
    let pool = &state.pool;
    let github = exists(
        pool,
        "SELECT 1 FROM github_installations WHERE project_id = $1 AND state = 'available' AND $2::uuid IS NOT NULL",
        project_id,
        actor_id,
    )
    .await?;
    let repository = exists(
        pool,
        "SELECT 1 FROM repositories WHERE project_id = $1 AND enabled AND archived_at IS NULL AND $2::uuid IS NOT NULL",
        project_id,
        actor_id,
    )
    .await?;
    let account = launch::launch_account(pool, project_id, actor_id, None, None)
        .await?
        .is_some();
    let environment = {
        let mut connection = pool.acquire().await?;
        launch::run_defaults(&mut connection, project_id, actor_id)
            .await?
            .environment
            .is_some()
    };
    let tasks = exists(
        pool,
        "SELECT 1 FROM task_rows WHERE project_id = $1 AND archived_at IS NULL AND $2::uuid IS NOT NULL",
        project_id,
        actor_id,
    )
    .await?;
    let agent = exists(
        pool,
        r#"
        SELECT 1 FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1 AND actor.kind = 'external_agent'
          AND membership.enabled AND $2::uuid IS NOT NULL
        "#,
        project_id,
        actor_id,
    )
    .await?;
    let launched = exists(
        pool,
        r#"
        SELECT 1 FROM launches WHERE project_id = $1 AND requested_by_actor_id = $2
        UNION ALL
        SELECT 1 FROM agent_runs run
        JOIN conversations thread ON thread.id = run.conversation_id
        WHERE thread.project_id = $1 AND run.requesting_actor_id = $2
        "#,
        project_id,
        actor_id,
    )
    .await?;
    Ok([
        ("github", github),
        ("repository", repository),
        ("account", account),
        ("environment", environment),
        ("tasks", tasks),
        ("agent", agent),
        ("launch", launched),
    ])
}

pub async fn onboarding_for(
    state: &AppState,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<Onboarding, AppError> {
    let (organization_name, role) = membership(&state.pool, organization_id, actor_id).await?;
    let progress = sqlx::query_as::<_, Progress>(
        r#"
        SELECT project_id, confirmed_steps, skipped_steps, dismissed_at IS NOT NULL AS dismissed
        FROM onboarding_progress WHERE organization_id = $1 AND actor_id = $2
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(&state.pool)
    .await?;
    let can_manage_workspace = matches!(role.as_str(), "owner" | "admin");
    let project = chosen_project(
        &state.pool,
        organization_id,
        actor_id,
        progress.as_ref().and_then(|progress| progress.project_id),
    )
    .await?;
    let confirmed = progress
        .as_ref()
        .map(|progress| progress.confirmed_steps.clone())
        .unwrap_or_default();
    let skipped = progress
        .as_ref()
        .map(|progress| progress.skipped_steps.clone())
        .unwrap_or_default();
    let used_template = can_manage_workspace
        && crate::workspace_templates::used_any(&state.pool, organization_id).await?;
    let mut done = vec![
        // Members don't pick the workspace's template, rename it or decide
        // its roles, so those steps aren't theirs to do.
        ("template", !can_manage_workspace || used_template),
        (
            "workspace",
            !can_manage_workspace || confirmed.iter().any(|step| step == "workspace"),
        ),
        (
            "roles",
            !can_manage_workspace || confirmed.iter().any(|step| step == "roles"),
        ),
        ("project", project.is_some()),
    ];
    match &project {
        Some((project_id, _)) => done.extend(project_steps(state, *project_id, actor_id).await?),
        None => done.extend(STEPS[4..].iter().map(|step| (*step, false))),
    }
    // AWD-75: steps of modules that are off don't show.
    let modules = {
        let mut connection = state.pool.acquire().await?;
        crate::modules::state::for_member(&mut connection, organization_id, actor_id).await?
    };
    let steps = done
        .into_iter()
        .filter(|(id, _)| modules.allows_setup_step(id))
        .map(|(id, done)| Step {
            id,
            done,
            skipped: !done && OPTIONAL.contains(&id) && skipped.iter().any(|step| step == id),
            optional: OPTIONAL.contains(&id),
        })
        .collect::<Vec<_>>();
    let next_step = steps
        .iter()
        .find(|step| !step.done && !step.skipped)
        .map(|step| step.id);
    Ok(Onboarding {
        organization_id,
        organization_name,
        project_id: project.as_ref().map(|(id, _)| *id),
        project_name: project.map(|(_, name)| name),
        complete: next_step.is_none(),
        next_step,
        steps,
        dismissed: progress.is_some_and(|progress| progress.dismissed),
        can_manage_workspace,
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRequest {
    /// The project the path sets up (one the person can open).
    #[serde(default)]
    project_id: Option<Uuid>,
    /// A step only the person's word ticks, such as naming the workspace.
    #[serde(default)]
    confirm: Option<String>,
    /// An optional step to skip, or to take back up with `false`.
    #[serde(default)]
    skip: Option<String>,
    #[serde(default)]
    unskip: Option<String>,
    /// Put the path away, or bring it back.
    #[serde(default)]
    dismissed: Option<bool>,
}

async fn read_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<Onboarding>, AppError> {
    Ok(Json(
        onboarding_for(&state, organization_id, principal.actor_id).await?,
    ))
}

async fn update_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<UpdateRequest>,
) -> Result<Json<Onboarding>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let actor_id = principal.actor_id;
    membership(&state.pool, organization_id, actor_id).await?;
    if let Some(project_id) = request.project_id {
        let reachable = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM projects
                WHERE id = $1 AND organization_id = $2 AND kind = 'standard'
                  AND archived_at IS NULL AND actor_can_access_project($3, NULL, id)
            )
            "#,
        )
        .bind(project_id)
        .bind(organization_id)
        .bind(actor_id)
        .fetch_one(&state.pool)
        .await?;
        if !reachable {
            return Err(AppError::NotFound("project"));
        }
    }
    if request
        .confirm
        .as_deref()
        .is_some_and(|step| !CONFIRMED.contains(&step))
    {
        return Err(AppError::Validation("confirm"));
    }
    for step in [request.skip.as_deref(), request.unskip.as_deref()]
        .into_iter()
        .flatten()
    {
        if !OPTIONAL.contains(&step) {
            return Err(AppError::Validation("skip"));
        }
    }
    sqlx::query(
        r#"
        INSERT INTO onboarding_progress (organization_id, actor_id, project_id, confirmed_steps,
                                         skipped_steps, dismissed_at)
        VALUES ($1, $2, $3,
                CASE WHEN $4::text IS NULL THEN '{}'::text[] ELSE ARRAY[$4::text] END,
                CASE WHEN $5::text IS NULL THEN '{}'::text[] ELSE ARRAY[$5::text] END,
                CASE WHEN $7::boolean THEN now() END)
        ON CONFLICT (organization_id, actor_id) DO UPDATE SET
            project_id = COALESCE($3, onboarding_progress.project_id),
            confirmed_steps = CASE
                WHEN $4::text IS NULL OR $4::text = ANY(onboarding_progress.confirmed_steps)
                THEN onboarding_progress.confirmed_steps
                ELSE array_append(onboarding_progress.confirmed_steps, $4::text) END,
            skipped_steps = array_remove(CASE
                WHEN $5::text IS NULL OR $5::text = ANY(onboarding_progress.skipped_steps)
                THEN onboarding_progress.skipped_steps
                ELSE array_append(onboarding_progress.skipped_steps, $5::text) END, $6::text),
            dismissed_at = CASE
                WHEN $7::boolean IS NULL THEN onboarding_progress.dismissed_at
                WHEN $7::boolean THEN COALESCE(onboarding_progress.dismissed_at, now())
                ELSE NULL END,
            updated_at = now()
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .bind(request.project_id)
    .bind(request.confirm.as_deref())
    .bind(request.skip.as_deref())
    .bind(request.unskip.as_deref())
    .bind(request.dismissed)
    .execute(&state.pool)
    .await?;
    Ok(Json(
        onboarding_for(&state, organization_id, actor_id).await?,
    ))
}

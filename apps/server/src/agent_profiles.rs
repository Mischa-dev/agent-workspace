//! Agent profiles (A9) and agent presence (A10).
//!
//! A profile is an agent people make, often from a template (Reviewer,
//! Planner, Fixer): a name and avatar, a harness and its default model
//! settings, instructions, allowed tools and an environment. Each profile is
//! its own execution worker in its project, with its own handle, so people
//! tag it (`@reviewer look at this PR`) and its runs post as it. Runs and
//! launches for a profile carry its settings; what a launch picks still
//! wins.
//!
//! Presence is read live from what agents are doing: a harness agent's
//! active runs (waiting for you when a run needs an answer), an external
//! agent's claimed requests and open sessions. What it's working on is only
//! named to people who can see that thread.

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, patch},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    environments::{authorize_project, principal_kind},
    error::{ApiJson, ApiPath, AppError},
    policy::{Action, ActorKind},
    settings,
};

const MAX_PROFILES_PER_PROJECT: i64 = 40;
const ACTIVE_RUN_STATES: &str =
    "('created', 'queued', 'starting', 'working', 'blocked', 'recovering')";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/agent-profiles",
            get(list_http).post(create_http),
        )
        .route(
            "/api/v1/projects/{project_id}/agent-profiles/{profile_id}",
            patch(update_http).delete(delete_http),
        )
        .route("/api/v1/projects/{project_id}/agents", get(presence_http))
}

// ---------------------------------------------------------------------------
// Templates

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileTemplate {
    pub id: &'static str,
    pub name: &'static str,
    pub handle: &'static str,
    pub avatar: &'static str,
    pub color: &'static str,
    pub description: &'static str,
    pub instructions: &'static str,
    /// Model settings as a run takes them; the harness's defaults fill the rest.
    pub run_defaults: Value,
}

pub fn templates() -> Vec<ProfileTemplate> {
    vec![
        ProfileTemplate {
            id: "reviewer",
            name: "Reviewer",
            handle: "reviewer",
            avatar: "R",
            color: "violet",
            description: "Reviews a pull request or a change: correctness, tests, security and readability. Comments with findings and doesn't change files.",
            instructions: "You are a code reviewer. Read the change you're pointed at (a pull request, a branch or the thread's run) and its surrounding code. Report what's wrong or risky, most important first: bugs and wrong behavior, missing or weak tests, security problems, then readability. Quote file and line for each finding and say how to fix it. Don't change files, commit or push. If the change looks right, say so plainly and name what you checked.",
            run_defaults: json!({ "reasoning": "high" }),
        },
        ProfileTemplate {
            id: "planner",
            name: "Planner",
            handle: "planner",
            avatar: "P",
            color: "teal",
            description: "Turns a goal into a plan: steps, open questions, risks and the tasks to create. Doesn't write code.",
            instructions: "You are a planner. Read the goal and the code it touches, then write a plan: the outcome, the steps in order with what each changes, open questions for people, risks, and a list of tasks small enough for one pull request each, with acceptance criteria. Don't change files, commit or push.",
            run_defaults: json!({ "reasoning": "high" }),
        },
        ProfileTemplate {
            id: "fixer",
            name: "Fixer",
            handle: "fixer",
            avatar: "F",
            color: "amber",
            description: "Fixes a bug or a failing test with the smallest change that does it, and proves it with a test run.",
            instructions: "You are a fixer. Reproduce the failure first, find its cause, and fix it with the smallest change that does. Add or update a test that fails without the fix. Run the tests that cover the change and say what you ran and what passed. Don't refactor unrelated code.",
            run_defaults: json!({}),
        },
    ]
}

// ---------------------------------------------------------------------------
// Views

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    pub id: Uuid,
    pub project_id: Uuid,
    pub actor_id: Uuid,
    pub name: String,
    pub handle: String,
    pub template: Option<String>,
    pub avatar: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub harness: Option<String>,
    #[sqlx(default)]
    pub harness_name: Option<String>,
    pub run_defaults: Value,
    pub instructions: String,
    pub allowed_tools: Vec<String>,
    pub environment_id: Option<Uuid>,
    pub environment_name: Option<String>,
    pub enabled: bool,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const PROFILE_SELECT: &str = r#"
    SELECT profile.id, profile.project_id, profile.actor_id,
           membership.display_name AS name, membership.handle,
           profile.template, profile.avatar, profile.color, profile.description,
           profile.harness, profile.run_defaults, profile.instructions,
           profile.allowed_tools, profile.environment_id,
           environment.name AS environment_name,
           profile.enabled, profile.version, profile.created_at, profile.updated_at
    FROM agent_profiles profile
    JOIN project_memberships membership
      ON membership.project_id = profile.project_id AND membership.actor_id = profile.actor_id
    LEFT JOIN project_environments environment
      ON environment.project_id = profile.project_id AND environment.id = profile.environment_id
"#;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessChoice {
    pub kind: String,
    pub display_name: String,
    /// Whether the harness takes instructions and an allowed-tools list
    /// (its options name `instructions` and `allowedTools`).
    pub takes_instructions: bool,
    pub takes_allowed_tools: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileList {
    pub profiles: Vec<ProfileView>,
    pub templates: Vec<ProfileTemplate>,
    pub harnesses: Vec<HarnessChoice>,
    pub can_manage: bool,
    /// Whether the project has a default agent to copy run access from;
    /// profiles need one (someone connects an account here first).
    pub ready: bool,
}

fn named(state: &AppState, mut profile: ProfileView) -> ProfileView {
    profile.harness_name = profile
        .harness
        .as_deref()
        .map(|harness| crate::agent_sessions::harness_name(state, harness));
    profile
}

fn takes(manifest: &crate::provider::manifest::AdapterManifest, option: &str) -> bool {
    manifest
        .options_schema
        .pointer(&format!("/properties/{option}"))
        .is_some()
}

pub async fn list_profiles(
    state: &AppState,
    viewer: Uuid,
    viewer_kind: ActorKind,
    project_id: Uuid,
) -> Result<ProfileList, AppError> {
    let organization_id = authorize_project(
        &state.pool,
        viewer,
        viewer_kind,
        project_id,
        Action::ProjectView,
    )
    .await?;
    let can_manage = viewer_kind == ActorKind::Human
        && crate::memory::project_access(
            &state.pool,
            viewer,
            viewer_kind,
            project_id,
            Action::SettingsManage,
        )
        .await
        .is_ok();
    let profiles = sqlx::query_as::<_, ProfileView>(&format!(
        "{PROFILE_SELECT} WHERE profile.project_id = $1 AND profile.hidden_at IS NULL ORDER BY profile.enabled DESC, membership.display_name, profile.id"
    ))
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|profile| named(state, profile))
    .collect();
    let harnesses = state
        .supervisors
        .manifests_in(organization_id)
        .into_iter()
        .filter(|manifest| state.supervisors.runner_for(&manifest.kind).is_none())
        .map(|manifest| HarnessChoice {
            kind: manifest.kind.clone(),
            display_name: manifest.display_name.clone(),
            takes_instructions: takes(&manifest, "instructions"),
            takes_allowed_tools: takes(&manifest, "allowedTools"),
        })
        .collect();
    let ready = default_worker(&state.pool, project_id).await?.is_some();
    Ok(ProfileList {
        profiles,
        templates: templates(),
        harnesses,
        can_manage,
        ready,
    })
}

async fn list_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ProfileList>, AppError> {
    let kind = principal_kind(&principal)?;
    Ok(Json(
        list_profiles(&state, principal.actor_id, kind, project_id).await?,
    ))
}

// ---------------------------------------------------------------------------
// Creating and changing

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileRequest {
    /// Start from a template; the fields given replace its own.
    pub template: Option<String>,
    pub name: Option<String>,
    pub handle: Option<String>,
    pub avatar: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    /// `Some(None)` (JSON null) goes back to the project's default harness.
    #[serde(default, deserialize_with = "double_option")]
    pub harness: Option<Option<String>>,
    pub run_defaults: Option<Map<String, Value>>,
    pub instructions: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub environment_id: Option<Option<Uuid>>,
    pub enabled: Option<bool>,
    /// The version the change was made against (updates only).
    pub version: Option<i64>,
}

fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A handle people can tag: lower case letters, digits, `-` and `_`.
pub fn handle_from(name: &str) -> String {
    let mut handle = String::new();
    for character in name.trim().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            handle.push(character);
        } else if matches!(character, '-' | '_' | ' ' | '.') && !handle.ends_with('-') {
            handle.push('-');
        }
        if handle.len() >= 28 {
            break;
        }
    }
    let handle = handle.trim_matches('-').to_owned();
    if handle.len() < 2 {
        format!("agent{handle}")
    } else {
        handle
    }
}

fn valid_handle(handle: &str) -> bool {
    let bytes = handle.as_bytes();
    (2..=32).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

struct Clean {
    name: String,
    handle: String,
    avatar: Option<String>,
    color: Option<String>,
    description: Option<String>,
    harness: Option<String>,
    run_defaults: Value,
    instructions: String,
    allowed_tools: Vec<String>,
    environment_id: Option<Uuid>,
}

fn text(value: Option<String>, max: usize, what: &'static str) -> Result<Option<String>, AppError> {
    match value.map(|value| value.trim().to_owned()) {
        None => Ok(None),
        Some(value) if value.is_empty() => Ok(None),
        Some(value) if value.chars().count() > max => Err(AppError::Validation(what)),
        Some(value) => Ok(Some(value)),
    }
}

async fn clean(
    state: &AppState,
    organization_id: Uuid,
    project_id: Uuid,
    request: ProfileRequest,
    current: Option<&ProfileView>,
) -> Result<Clean, AppError> {
    let template = match request.template.as_deref() {
        Some(id) => Some(
            templates()
                .into_iter()
                .find(|template| template.id == id)
                .ok_or(AppError::Validation("template"))?,
        ),
        None => None,
    };
    let name = text(request.name, 80, "name")?
        .or_else(|| current.map(|profile| profile.name.clone()))
        .or_else(|| template.as_ref().map(|template| template.name.to_owned()))
        .ok_or(AppError::Validation("name"))?;
    let handle = match request.handle {
        Some(handle) => handle.trim().trim_start_matches('@').to_lowercase(),
        None => current.map_or_else(
            || {
                template
                    .as_ref()
                    .map_or_else(|| handle_from(&name), |template| template.handle.to_owned())
            },
            |profile| profile.handle.clone(),
        ),
    };
    if !valid_handle(&handle) {
        return Err(AppError::Validation("handle"));
    }
    let avatar = match request.avatar {
        Some(avatar) => text(Some(avatar), 8, "avatar")?,
        None => current
            .and_then(|profile| profile.avatar.clone())
            .or_else(|| template.as_ref().map(|template| template.avatar.to_owned())),
    };
    let color = match request.color {
        Some(color) => {
            let color = color.trim().to_lowercase();
            if !(3..=12).contains(&color.len())
                || !color.bytes().all(|byte| byte.is_ascii_lowercase())
            {
                return Err(AppError::Validation("color"));
            }
            Some(color)
        }
        None => current
            .and_then(|profile| profile.color.clone())
            .or_else(|| template.as_ref().map(|template| template.color.to_owned())),
    };
    let description = match request.description {
        Some(description) => text(Some(description), 500, "description")?,
        None => current
            .and_then(|profile| profile.description.clone())
            .or_else(|| {
                template
                    .as_ref()
                    .map(|template| template.description.to_owned())
            }),
    };
    let harness = match request.harness {
        Some(harness) => harness.filter(|harness| !harness.trim().is_empty()),
        None => current.and_then(|profile| profile.harness.clone()),
    };
    if let Some(harness) = &harness {
        let known = state
            .supervisors
            .manifest_in(organization_id, harness)
            .is_some();
        if !known || state.supervisors.runner_for(harness).is_some() {
            return Err(AppError::Validation("harness"));
        }
    }
    let run_defaults = match request.run_defaults {
        Some(defaults) => {
            let defaults = defaults
                .into_iter()
                .filter(|(key, _)| matches!(key.as_str(), "model" | "reasoning" | "speed"))
                .map(|(key, value)| match value {
                    Value::String(text) if text.len() <= 200 => Ok((key, Value::String(text))),
                    Value::Null => Ok((key, Value::Null)),
                    _ => Err(AppError::Validation("runDefaults")),
                })
                .collect::<Result<Map<_, _>, _>>()?;
            Value::Object(
                defaults
                    .into_iter()
                    .filter(|(_, value)| !value.is_null())
                    .collect(),
            )
        }
        None => current.map_or_else(
            || {
                template
                    .as_ref()
                    .map_or_else(|| json!({}), |template| template.run_defaults.clone())
            },
            |profile| profile.run_defaults.clone(),
        ),
    };
    let instructions = match request.instructions {
        Some(instructions) => {
            let instructions = instructions.trim().to_owned();
            if instructions.chars().count() > 8_000 {
                return Err(AppError::Validation("instructions"));
            }
            instructions
        }
        None => current.map_or_else(
            || {
                template
                    .as_ref()
                    .map(|template| template.instructions.to_owned())
                    .unwrap_or_default()
            },
            |profile| profile.instructions.clone(),
        ),
    };
    let allowed_tools = match request.allowed_tools {
        Some(tools) => {
            let mut cleaned = Vec::new();
            for tool in tools {
                let tool = tool.trim().to_owned();
                if tool.is_empty() {
                    continue;
                }
                if tool.chars().count() > 200 || tool.chars().any(char::is_control) {
                    return Err(AppError::Validation("allowedTools"));
                }
                if !cleaned.contains(&tool) {
                    cleaned.push(tool);
                }
            }
            if cleaned.len() > 64 {
                return Err(AppError::Validation("allowedTools"));
            }
            cleaned
        }
        None => current.map_or_else(Vec::new, |profile| profile.allowed_tools.clone()),
    };
    let environment_id = match request.environment_id {
        Some(environment_id) => environment_id,
        None => current.and_then(|profile| profile.environment_id),
    };
    if let Some(environment_id) = environment_id
        && crate::environments::active_environment(&state.pool, project_id, environment_id)
            .await?
            .is_none()
    {
        return Err(AppError::NotFound("environment"));
    }
    Ok(Clean {
        name,
        handle,
        avatar,
        color,
        description,
        harness,
        run_defaults,
        instructions,
        allowed_tools,
        environment_id,
    })
}

/// The project's default `@agent` worker, whose run access a profile copies.
async fn default_worker(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
) -> Result<Option<(Uuid, Uuid, bool, bool)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, Uuid, bool, bool)>(
        r#"
        SELECT worker.provider_connection_id, worker.execution_profile_id,
               worker.enabled, worker.pending_setup
        FROM execution_workers worker
        JOIN actors actor ON actor.id = worker.actor_id AND actor.kind = 'execution_worker'
        WHERE worker.project_id = $1
          AND NOT EXISTS (
              SELECT 1 FROM agent_profiles profile WHERE profile.execution_worker_id = worker.id
          )
        ORDER BY worker.created_at, worker.id
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .fetch_optional(executor)
    .await?)
}

async fn handle_taken(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    handle: &str,
    except_actor: Option<Uuid>,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM project_memberships
            WHERE project_id = $1 AND enabled AND lower(handle) = lower($2)
              AND ($3::uuid IS NULL OR actor_id <> $3)
        )
        "#,
    )
    .bind(project_id)
    .bind(handle)
    .bind(except_actor)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn manage(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<Uuid, AppError> {
    require_authenticated_write(state, headers, principal)?;
    if principal.actor_kind != "human" {
        return Err(AppError::Forbidden);
    }
    authorize_project(
        &state.pool,
        principal.actor_id,
        ActorKind::Human,
        project_id,
        Action::SettingsManage,
    )
    .await
}

pub(crate) async fn load(
    executor: impl sqlx::PgExecutor<'_>,
    state: &AppState,
    project_id: Uuid,
    profile_id: Uuid,
) -> Result<ProfileView, AppError> {
    sqlx::query_as::<_, ProfileView>(&format!(
        "{PROFILE_SELECT} WHERE profile.project_id = $1 AND profile.id = $2 AND profile.hidden_at IS NULL"
    ))
    .bind(project_id)
    .bind(profile_id)
    .fetch_optional(executor)
    .await?
    .map(|profile| named(state, profile))
    .ok_or(AppError::NotFound("agent_profile"))
}

async fn create_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ProfileRequest>,
) -> Result<Json<ProfileView>, AppError> {
    let organization_id = manage(&state, &headers, &principal, project_id).await?;
    let mut transaction = state.pool.begin().await?;
    let (profile_id, metadata) = create_in(
        &state,
        &mut transaction,
        principal.actor_id,
        organization_id,
        project_id,
        request,
    )
    .await?;
    settings::audit(
        &mut transaction,
        &headers,
        &principal,
        Some(project_id),
        "agent_profile.created",
        "agent_profile",
        Some(profile_id),
        metadata,
    )
    .await?;
    let view = load(&mut *transaction, &state, project_id, profile_id).await?;
    transaction.commit().await?;
    Ok(Json(view))
}

/// Make an agent in the project, for someone the caller checked may manage
/// its settings. Returns the profile and the audit row's metadata; the
/// caller records the row.
pub(crate) async fn create_in(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    caller_id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    request: ProfileRequest,
) -> Result<(Uuid, Value), AppError> {
    let clean = clean(state, organization_id, project_id, request.clone(), None).await?;
    crate::execution_setup::lock_project_setup(transaction, project_id).await?;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM agent_profiles WHERE project_id = $1 AND hidden_at IS NULL",
    )
    .bind(project_id)
    .fetch_one(&mut **transaction)
    .await?;
    if count >= MAX_PROFILES_PER_PROJECT {
        return Err(AppError::Conflict(
            "agent_profile",
            "this project has as many agents as it can hold; remove one first",
        ));
    }
    let Some((connection_id, execution_profile_id, enabled, pending_setup)) =
        default_worker(&mut **transaction, project_id).await?
    else {
        return Err(AppError::Conflict(
            "agent_profile",
            "connect a harness account in this project first, so its agents can run",
        ));
    };
    // A taken handle gets a number, as a second Reviewer does.
    let mut handle = clean.handle.clone();
    let mut suffix = 2;
    while handle_taken(transaction, project_id, &handle, None).await? {
        if request.handle.is_some() {
            return Err(AppError::Conflict(
                "agent_profile",
                "someone in this project already has that handle",
            ));
        }
        handle = format!(
            "{}-{suffix}",
            clean.handle.chars().take(28).collect::<String>()
        );
        suffix += 1;
        if suffix > 50 {
            return Err(AppError::Validation("handle"));
        }
    }
    let actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'execution_worker')")
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
        VALUES ($1, $2, $3, 'worker', $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .bind(&clean.name)
    .bind(&handle)
    .execute(&mut **transaction)
    .await?;
    let worker_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO execution_workers (
            id, project_id, actor_id, provider_connection_id, execution_profile_id,
            enabled, pending_setup
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(worker_id)
    .bind(project_id)
    .bind(actor_id)
    .bind(connection_id)
    .bind(execution_profile_id)
    .bind(enabled)
    .bind(pending_setup)
    .execute(&mut **transaction)
    .await?;
    let profile_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_profiles (
            id, organization_id, project_id, actor_id, execution_worker_id, template,
            avatar, color, description, harness, run_defaults, instructions,
            allowed_tools, environment_id, created_by_actor_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        "#,
    )
    .bind(profile_id)
    .bind(organization_id)
    .bind(project_id)
    .bind(actor_id)
    .bind(worker_id)
    .bind(request.template.as_deref())
    .bind(&clean.avatar)
    .bind(&clean.color)
    .bind(&clean.description)
    .bind(&clean.harness)
    .bind(&clean.run_defaults)
    .bind(&clean.instructions)
    .bind(&clean.allowed_tools)
    .bind(clean.environment_id)
    .bind(caller_id)
    .execute(&mut **transaction)
    .await?;
    Ok((
        profile_id,
        json!({ "template": request.template, "handle": handle, "harness": clean.harness }),
    ))
}

async fn update_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<ProfileRequest>,
) -> Result<Json<ProfileView>, AppError> {
    let organization_id = manage(&state, &headers, &principal, project_id).await?;
    let mut transaction = state.pool.begin().await?;
    let (action, metadata) = update_in(
        &state,
        &mut transaction,
        principal.actor_id,
        organization_id,
        project_id,
        profile_id,
        request,
    )
    .await?;
    settings::audit(
        &mut transaction,
        &headers,
        &principal,
        Some(project_id),
        action,
        "agent_profile",
        Some(profile_id),
        metadata,
    )
    .await?;
    let view = load(&mut *transaction, &state, project_id, profile_id).await?;
    transaction.commit().await?;
    Ok(Json(view))
}

/// Change an agent of the project (turning it off takes it out of
/// mentions), for someone the caller checked may manage its settings.
/// Returns the audit row's action and metadata; the caller records it.
pub(crate) async fn update_in(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    profile_id: Uuid,
    request: ProfileRequest,
) -> Result<(&'static str, Value), AppError> {
    if request.template.is_some() {
        return Err(AppError::Validation("template"));
    }
    let current = load(&mut **transaction, state, project_id, profile_id).await?;
    if request
        .version
        .is_some_and(|version| version != current.version)
    {
        return Err(AppError::Conflict(
            "agent_profile",
            "someone changed this agent since you opened it; reload and try again",
        ));
    }
    let enabled = request.enabled.unwrap_or(current.enabled);
    let clean = clean(state, organization_id, project_id, request, Some(&current)).await?;
    crate::execution_setup::lock_project_setup(transaction, project_id).await?;
    if enabled
        && handle_taken(
            transaction,
            project_id,
            &clean.handle,
            Some(current.actor_id),
        )
        .await?
    {
        return Err(AppError::Conflict(
            "agent_profile",
            "someone in this project already has that handle",
        ));
    }
    let updated = sqlx::query(
        r#"
        UPDATE agent_profiles
        SET avatar = $3, color = $4, description = $5, harness = $6, run_defaults = $7,
            instructions = $8, allowed_tools = $9, environment_id = $10, enabled = $11,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND version = $12
        "#,
    )
    .bind(project_id)
    .bind(profile_id)
    .bind(&clean.avatar)
    .bind(&clean.color)
    .bind(&clean.description)
    .bind(&clean.harness)
    .bind(&clean.run_defaults)
    .bind(&clean.instructions)
    .bind(&clean.allowed_tools)
    .bind(clean.environment_id)
    .bind(enabled)
    .bind(current.version)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if updated != 1 {
        return Err(AppError::Conflict(
            "agent_profile",
            "someone changed this agent since you opened it; reload and try again",
        ));
    }
    // Turning an agent off takes it out of mentions and stops new runs; its
    // past runs and messages stay.
    sqlx::query(
        r#"
        UPDATE project_memberships
        SET display_name = $3, handle = $4, enabled = $5,
            disabled_by_actor_id = CASE WHEN $5 THEN NULL ELSE $6 END,
            disabled_at = CASE WHEN $5 THEN NULL ELSE COALESCE(disabled_at, now()) END,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND actor_id = $2
        "#,
    )
    .bind(project_id)
    .bind(current.actor_id)
    .bind(&clean.name)
    .bind(&clean.handle)
    .bind(enabled)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE execution_workers
        SET enabled = $3 AND NOT pending_setup, version = version + 1, updated_at = now()
        WHERE project_id = $1 AND actor_id = $2
        "#,
    )
    .bind(project_id)
    .bind(current.actor_id)
    .bind(enabled)
    .execute(&mut **transaction)
    .await?;
    Ok((
        if enabled == current.enabled {
            "agent_profile.updated"
        } else if enabled {
            "agent_profile.enabled"
        } else {
            "agent_profile.disabled"
        },
        json!({ "handle": clean.handle, "harness": clean.harness }),
    ))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedProfile {
    /// `deleted`, or `hidden` when runs, launches or messages still point at
    /// it: then it's off the list, out of mentions and can't run, and what it
    /// did keeps its name.
    pub outcome: &'static str,
}

/// Deletes an agent that never did anything; one that did is hidden instead,
/// so its past runs and messages keep their author.
async fn delete_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, profile_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<RemovedProfile>, AppError> {
    manage(&state, &headers, &principal, project_id).await?;
    let mut transaction = state.pool.begin().await?;
    let (outcome, handle) =
        remove_in(&mut transaction, principal.actor_id, project_id, profile_id).await?;
    settings::audit(
        &mut transaction,
        &headers,
        &principal,
        Some(project_id),
        &format!("agent_profile.{outcome}"),
        "agent_profile",
        Some(profile_id),
        json!({ "handle": handle }),
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(RemovedProfile { outcome }))
}

/// Remove an agent of the project: delete it if it never did anything, or
/// hide it. Returns the outcome ("deleted" or "hidden") and its handle for
/// the caller's audit row.
pub(crate) async fn remove_in(
    transaction: &mut Transaction<'_, Postgres>,
    caller_id: Uuid,
    project_id: Uuid,
    profile_id: Uuid,
) -> Result<(&'static str, String), AppError> {
    crate::execution_setup::lock_project_setup(transaction, project_id).await?;
    let (actor_id, worker_id, handle) = sqlx::query_as::<_, (Uuid, Uuid, String)>(
        r#"
        SELECT profile.actor_id, profile.execution_worker_id, membership.handle
        FROM agent_profiles profile
        JOIN project_memberships membership
          ON membership.project_id = profile.project_id AND membership.actor_id = profile.actor_id
        WHERE profile.project_id = $1 AND profile.id = $2 AND profile.hidden_at IS NULL
        FOR UPDATE OF profile
        "#,
    )
    .bind(project_id)
    .bind(profile_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("agent_profile"))?;
    let (running, used) = sqlx::query_as::<_, (bool, bool)>(&format!(
        r#"
        SELECT
            EXISTS (
                SELECT 1 FROM agent_runs
                WHERE execution_worker_id = $1 AND state IN {ACTIVE_RUN_STATES}
            ),
            EXISTS (SELECT 1 FROM agent_runs WHERE execution_worker_id = $1)
                OR EXISTS (SELECT 1 FROM launches WHERE agent_profile_id = $2)
        "#
    ))
    .bind(worker_id)
    .bind(profile_id)
    .fetch_one(&mut **transaction)
    .await?;
    if running {
        return Err(AppError::Conflict(
            "agent_profile",
            "this agent is working on something; stop its runs first",
        ));
    }
    // Anything else that names it (a message it wrote, a mention, a
    // reaction) refuses the delete, and it's hidden instead.
    let deleted = !used && {
        let mut attempt = sqlx::Acquire::begin(&mut **transaction).await?;
        match delete_profile_rows(&mut attempt, project_id, profile_id, worker_id, actor_id).await {
            Ok(()) => {
                attempt.commit().await?;
                true
            }
            Err(sqlx::Error::Database(error)) if error.is_foreign_key_violation() => {
                attempt.rollback().await?;
                false
            }
            Err(error) => return Err(error.into()),
        }
    };
    if !deleted {
        sqlx::query(
            r#"
            UPDATE agent_profiles
            SET hidden_at = now(), enabled = false, version = version + 1, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(profile_id)
        .execute(&mut **transaction)
        .await?;
        sqlx::query(
            r#"
            UPDATE project_memberships
            SET enabled = false, disabled_by_actor_id = $3,
                disabled_at = COALESCE(disabled_at, now()),
                version = version + 1, updated_at = now()
            WHERE project_id = $1 AND actor_id = $2
            "#,
        )
        .bind(project_id)
        .bind(actor_id)
        .bind(caller_id)
        .execute(&mut **transaction)
        .await?;
        sqlx::query(
            r#"
            UPDATE execution_workers SET enabled = false, version = version + 1, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(worker_id)
        .execute(&mut **transaction)
        .await?;
    }
    let outcome = if deleted { "deleted" } else { "hidden" };
    Ok((outcome, handle))
}

async fn delete_profile_rows(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    profile_id: Uuid,
    worker_id: Uuid,
    actor_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM agent_profiles WHERE id = $1")
        .bind(profile_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM execution_workers WHERE id = $1")
        .bind(worker_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM project_memberships WHERE project_id = $1 AND actor_id = $2")
        .bind(project_id)
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM actors WHERE id = $1 AND kind = 'execution_worker'")
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Runs and launches for a profile

/// The execution worker of an enabled profile in the project.
pub(crate) async fn profile_worker(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    profile_id: Uuid,
) -> Result<Uuid, AppError> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT execution_worker_id FROM agent_profiles WHERE project_id = $1 AND id = $2 AND enabled",
    )
    .bind(project_id)
    .bind(profile_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("agent_profile"))
}

/// What a profile gives a launch: its harness, environment, model settings
/// and the options its harness takes.
#[derive(Clone, Debug, FromRow)]
pub(crate) struct ProfileSettings {
    pub id: Uuid,
    pub harness: Option<String>,
    pub environment_id: Option<Uuid>,
    pub run_defaults: Value,
    pub instructions: String,
    pub allowed_tools: Vec<String>,
}

impl ProfileSettings {
    fn default_text(&self, key: &str) -> Option<String> {
        self.run_defaults
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    pub fn model(&self) -> Option<String> {
        self.default_text("model")
    }

    pub fn reasoning(&self) -> Option<String> {
        self.default_text("reasoning")
    }

    pub fn speed(&self) -> Option<String> {
        self.default_text("speed")
    }

    /// The options the harness takes from the profile: its instructions and
    /// allowed tools, for harnesses whose options name them.
    pub fn options(
        &self,
        manifest: &crate::provider::manifest::AdapterManifest,
    ) -> Map<String, Value> {
        let mut options = Map::new();
        if !self.instructions.trim().is_empty() && takes(manifest, "instructions") {
            options.insert("instructions".to_owned(), json!(self.instructions));
        }
        if !self.allowed_tools.is_empty() && takes(manifest, "allowedTools") {
            options.insert("allowedTools".to_owned(), json!(self.allowed_tools));
        }
        options
    }

    /// For a harness that can't take instructions, they go ahead of the
    /// prompt instead, so every harness hears them.
    pub fn prompt(
        &self,
        manifest: Option<&crate::provider::manifest::AdapterManifest>,
        prompt: &str,
    ) -> String {
        let instructions = self.instructions.trim();
        if instructions.is_empty()
            || manifest.is_some_and(|manifest| takes(manifest, "instructions"))
        {
            return prompt.to_owned();
        }
        format!("{instructions}\n\n---\n\n{prompt}")
    }
}

pub(crate) async fn profile_settings(
    executor: impl sqlx::PgExecutor<'_>,
    profile_id: Uuid,
) -> Result<Option<ProfileSettings>, AppError> {
    Ok(sqlx::query_as::<_, ProfileSettings>(
        r#"
        SELECT id, harness, environment_id, run_defaults, instructions, allowed_tools
        FROM agent_profiles WHERE id = $1 AND enabled
        "#,
    )
    .bind(profile_id)
    .fetch_optional(executor)
    .await?)
}

/// The profile behind a worker actor, for a tag.
pub(crate) async fn profile_for_actor(
    executor: impl sqlx::PgExecutor<'_>,
    actor_id: Uuid,
) -> Result<Option<ProfileSettings>, AppError> {
    Ok(sqlx::query_as::<_, ProfileSettings>(
        r#"
        SELECT id, harness, environment_id, run_defaults, instructions, allowed_tools
        FROM agent_profiles WHERE actor_id = $1 AND enabled
        "#,
    )
    .bind(actor_id)
    .fetch_optional(executor)
    .await?)
}

// ---------------------------------------------------------------------------
// Presence (A10)

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresence {
    pub actor_id: Uuid,
    pub name: String,
    pub handle: String,
    /// worker (runs on a harness here) or agent (connects on its own).
    pub kind: String,
    pub profile_id: Option<Uuid>,
    pub avatar: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub harness: Option<String>,
    /// idle, working or waiting (for an answer from someone).
    pub state: String,
    /// How many things it's on now.
    pub active: i64,
    pub since: Option<DateTime<Utc>>,
    /// What it's working on, for people who can see that thread.
    pub conversation_id: Option<Uuid>,
    pub thread_name: Option<String>,
    pub run_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub task_key: Option<String>,
    pub task_title: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceList {
    pub agents: Vec<AgentPresence>,
}

/// Every agent in the project and what it's doing now.
pub async fn presence(
    state: &AppState,
    viewer: Uuid,
    viewer_kind: ActorKind,
    project_id: Uuid,
) -> Result<PresenceList, AppError> {
    authorize_project(
        &state.pool,
        viewer,
        viewer_kind,
        project_id,
        Action::ProjectView,
    )
    .await?;
    let agents = sqlx::query_as::<_, AgentPresence>(&format!(
        r#"
        WITH members AS (
            SELECT membership.actor_id, membership.display_name AS name, membership.handle,
                   actor.kind AS actor_kind
            FROM project_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
            WHERE membership.project_id = $1 AND membership.enabled
              AND actor.kind IN ('execution_worker', 'external_agent')
        ),
        work AS (
            -- A harness agent's runs: blocked ones first, they wait for you.
            SELECT worker.actor_id, run.conversation_id, run.id AS run_id,
                   run.state = 'blocked' AS waiting, COALESCE(run.started_at, run.created_at) AS since
            FROM agent_runs run
            JOIN execution_workers worker ON worker.id = run.execution_worker_id
            WHERE worker.project_id = $1 AND run.state IN {ACTIVE_RUN_STATES}
            UNION ALL
            -- An external agent's claimed requests.
            SELECT request.agent_actor_id, COALESCE(request.thread_id, request.conversation_id),
                   NULL, false, request.updated_at
            FROM reply_requests request
            WHERE request.project_id = $1 AND request.status = 'claimed'
              AND request.lease_expires_at > now()
            UNION ALL
            -- Sessions a runner keeps open without a claim.
            SELECT session.agent_actor_id, session.conversation_id, NULL, false,
                   session.last_activity_at
            FROM agent_sessions session
            WHERE session.project_id = $1 AND session.state = 'active'
              AND session.source = 'runner'
              AND session.last_activity_at > now() - interval '30 minutes'
        ),
        ranked AS (
            SELECT work.*, count(*) OVER (PARTITION BY work.actor_id) AS active,
                   bool_or(work.waiting) OVER (PARTITION BY work.actor_id) AS any_waiting,
                   row_number() OVER (
                       PARTITION BY work.actor_id
                       ORDER BY conversation_visible_to(work.conversation_id, $2) DESC,
                                work.waiting DESC, work.since DESC
                   ) AS position
            FROM work
        )
        SELECT member.actor_id, member.name, member.handle,
               CASE WHEN member.actor_kind = 'execution_worker' THEN 'worker' ELSE 'agent' END AS kind,
               profile.id AS profile_id, profile.avatar, profile.color, profile.description,
               profile.harness,
               CASE WHEN ranked.actor_id IS NULL THEN 'idle'
                    WHEN ranked.any_waiting THEN 'waiting'
                    ELSE 'working' END AS state,
               COALESCE(ranked.active, 0) AS active,
               ranked.since,
               visible.conversation_id, visible.thread_name, visible.run_id,
               task.id AS task_id,
               CASE WHEN task.id IS NOT NULL THEN project.project_key || '-' || task.number END AS task_key,
               task.title AS task_title
        FROM members member
        JOIN projects project ON project.id = $1
        LEFT JOIN agent_profiles profile
          ON profile.project_id = $1 AND profile.actor_id = member.actor_id
        LEFT JOIN ranked ON ranked.actor_id = member.actor_id AND ranked.position = 1
        LEFT JOIN LATERAL (
            SELECT ranked.conversation_id, conversation.name AS thread_name, ranked.run_id
            FROM conversations conversation
            WHERE conversation.id = ranked.conversation_id
              AND conversation_visible_to(ranked.conversation_id, $2)
        ) visible ON true
        LEFT JOIN LATERAL (
            SELECT link.work_item_id FROM work_item_conversation_links link
            WHERE link.project_id = $1 AND link.conversation_id = visible.conversation_id
            ORDER BY link.created_at, link.work_item_id LIMIT 1
        ) task_link ON true
        LEFT JOIN task_rows task ON task.id = task_link.work_item_id AND task.archived_at IS NULL
        WHERE profile.id IS NULL OR profile.enabled
        ORDER BY (ranked.actor_id IS NULL), member.name, member.actor_id
        LIMIT 200
        "#
    ))
    .bind(project_id)
    .bind(viewer)
    .fetch_all(&state.pool)
    .await?;
    Ok(PresenceList { agents })
}

async fn presence_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<PresenceList>, AppError> {
    let kind = principal_kind(&principal)?;
    Ok(Json(
        presence(&state, principal.actor_id, kind, project_id).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_come_from_names() {
        assert_eq!(handle_from("Reviewer"), "reviewer");
        assert_eq!(handle_from("Release  Notes Bot!"), "release-notes-bot");
        assert_eq!(handle_from("Ä"), "agent");
        assert!(valid_handle(&handle_from(
            "A very long agent name that keeps going on"
        )));
        assert!(!valid_handle("-bad"));
        assert!(!valid_handle("Upper"));
    }

    #[test]
    fn templates_fit_the_table() {
        for template in templates() {
            assert!(valid_handle(template.handle));
            assert!(template.instructions.len() <= 8_000);
            assert!(template.description.len() <= 500);
            assert!((3..=12).contains(&template.color.len()));
        }
    }
}

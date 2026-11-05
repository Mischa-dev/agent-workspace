//! The Modules routes: what's on in a workspace (anyone in it can read),
//! switching modules or applying a preset (owners and admins, audited with
//! before and after), and opening any thing with its links.

use std::collections::BTreeMap;

use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{
    ModuleSet,
    contract::{Thing, ThingLink, ToolVerb},
    manifest::{self, Manifest, ModuleKind, ModulePreset},
    state, things,
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    settings::{self, Target},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/organizations/{organization_id}/modules",
            get(read_modules).put(change_modules),
        )
        .route("/api/v1/things/{kind}/{id}", get(open_thing))
}

/// One module as a settings screen and an agent read it.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModuleView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: ModuleKind,
    pub switchable: bool,
    /// Whether it's on for the reader.
    pub on: bool,
    /// Whether the workspace's switch is on (it may still be off because a
    /// module it needs is off).
    pub switched_on: bool,
    /// Why it's off, in words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub off_because: Option<String>,
    pub needs: Vec<String>,
    pub works_with: Vec<String>,
    /// The kinds of thing it makes.
    pub things: Vec<String>,
    /// Its agent tools, by what they do.
    pub tools: BTreeMap<ToolVerb, Vec<String>>,
    /// Live blocks it offers for pages.
    pub embeds: Vec<String>,
    /// The events it sends.
    pub events: Vec<String>,
    /// The switch setting's version, for expectedVersions.
    pub version: i64,
}

impl ModuleView {
    pub fn of(module: &Manifest, set: ModuleSet, switches: &state::Switches) -> Self {
        let switched_off = switches.is_off(&module.id);
        let mut tools: BTreeMap<ToolVerb, Vec<String>> = BTreeMap::new();
        for (tool, verb) in &module.tools {
            tools.entry(*verb).or_default().push(tool.clone());
        }
        Self {
            id: module.id.clone(),
            name: module.name.clone(),
            description: module.description.clone(),
            kind: module.kind,
            switchable: module.switchable,
            on: set.contains(&module.id),
            switched_on: !switched_off,
            off_because: set.why_off(module, switched_off),
            needs: module.needs.clone(),
            works_with: module.works_with.clone(),
            things: module
                .things
                .iter()
                .map(|thing| thing.kind.clone())
                .collect(),
            tools,
            embeds: module.embeds.iter().map(|embed| embed.id.clone()).collect(),
            events: module.events.emits.clone(),
            version: switches.version(&module.id),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModulesResponse {
    organization_id: Uuid,
    modules: Vec<ModuleView>,
    presets: &'static [ModulePreset],
    /// The preset the switches match exactly, if any.
    preset: Option<String>,
    can_manage: bool,
}

/// The preset a workspace's switches match, if one does.
pub fn matching_preset(switches: &state::Switches) -> Option<String> {
    manifest::presets()
        .iter()
        .find(|preset| {
            manifest::core()
                .iter()
                .filter(|module| module.switchable)
                .all(|module| preset.on.contains(&module.id) != switches.is_off(&module.id))
        })
        .map(|preset| preset.id.clone())
}

async fn member_role(
    connection: &mut sqlx::PgConnection,
    principal: &AuthenticatedActor,
    organization_id: Uuid,
) -> Result<String, AppError> {
    sqlx::query_scalar::<_, String>(
        r#"
        SELECT membership.role
        FROM organization_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
        WHERE membership.organization_id = $1 AND membership.actor_id = $2
          AND membership.enabled
        "#,
    )
    .bind(organization_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("workspace"))
}

async fn view(
    connection: &mut sqlx::PgConnection,
    principal: &AuthenticatedActor,
    organization_id: Uuid,
) -> Result<ModulesResponse, AppError> {
    let role = member_role(connection, principal, organization_id).await?;
    let switches = state::switches(connection, organization_id).await?;
    let set = state::for_member(connection, organization_id, principal.actor_id).await?;
    Ok(ModulesResponse {
        organization_id,
        modules: manifest::core()
            .iter()
            .map(|module| ModuleView::of(module, set, &switches))
            .collect(),
        presets: manifest::presets(),
        preset: matching_preset(&switches),
        can_manage: principal.actor_kind == "human" && matches!(role.as_str(), "owner" | "admin"),
    })
}

/// `GET /api/v1/organizations/{organization_id}/modules`: every module, and
/// whether it's on for the reader.
async fn read_modules(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<ModulesResponse>, AppError> {
    let mut connection = state.pool.acquire().await?;
    Ok(Json(
        view(&mut connection, &principal, organization_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangeRequest {
    /// Start from a preset: its modules on, the rest off.
    #[serde(default)]
    preset: Option<String>,
    /// Then switch these, by module id.
    #[serde(default)]
    switches: BTreeMap<String, bool>,
}

/// `PUT /api/v1/organizations/{organization_id}/modules`: owners and admins.
async fn change_modules(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<ChangeRequest>,
) -> Result<Json<ModulesResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut wanted = BTreeMap::new();
    if let Some(preset) = &request.preset {
        let preset = manifest::preset(preset).ok_or(AppError::Validation("preset"))?;
        for module in manifest::core().iter().filter(|module| module.switchable) {
            wanted.insert(module.id.clone(), preset.on.contains(&module.id));
        }
    }
    for (module, on) in &request.switches {
        let found = manifest::get(module).ok_or(AppError::Validation("module"))?;
        if !found.switchable {
            return Err(AppError::Validation("module can't be switched"));
        }
        wanted.insert(module.clone(), *on);
    }
    if wanted.is_empty() {
        return Err(AppError::Validation("switches"));
    }
    let mut transaction = state.pool.begin().await?;
    settings::authorize_workspace_manage(&mut *transaction, &principal, organization_id).await?;
    let before = state::switches(&mut transaction, organization_id).await?;
    let mut changed = BTreeMap::new();
    for (module, on) in &wanted {
        if before.is_off(module) == !on {
            continue;
        }
        let target = Target::Workspace { organization_id };
        let key = state::setting_key(module);
        if *on {
            // On is the default: clear the switch rather than store it.
            settings::reset(&mut transaction, target, &key, None).await?;
        } else {
            settings::set(
                &mut transaction,
                target,
                &key,
                json!(false),
                None,
                principal.actor_id,
            )
            .await?;
        }
        changed.insert(module.clone(), *on);
    }
    if !changed.is_empty() {
        let after = state::switches(&mut transaction, organization_id).await?;
        let off = |switches: &state::Switches| {
            manifest::core()
                .iter()
                .filter(|module| switches.is_off(&module.id))
                .map(|module| module.id.clone())
                .collect::<Vec<_>>()
        };
        settings::audit(
            &mut transaction,
            &headers,
            &principal,
            None,
            "modules.changed",
            "organization",
            Some(organization_id),
            json!({
                "organizationId": organization_id,
                "preset": request.preset,
                "changed": changed,
                "before": { "off": off(&before) },
                "after": { "off": off(&after) },
            }),
        )
        .await?;
    }
    let response = view(&mut transaction, &principal, organization_id).await?;
    transaction.commit().await?;
    Ok(Json(response))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThingResponse {
    thing: Thing,
    links: Vec<ThingLink>,
}

/// `GET /api/v1/things/{kind}/{id}`: a thing and its links both ways, as
/// far as the reader can see.
async fn open_thing(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
) -> Result<Json<ThingResponse>, AppError> {
    if !things::KINDS.contains(&kind.as_str()) {
        return Err(AppError::NotFound("thing"));
    }
    let projects = crate::search::browser_projects(&state.pool, principal.actor_id).await?;
    let mut connection = state.pool.acquire().await?;
    let modules = state::anywhere(&mut connection, principal.actor_id).await?;
    let viewer = things::Viewer {
        actor_id: principal.actor_id,
        projects: &projects,
        modules,
    };
    let thing = things::get(&mut connection, &viewer, &kind, id)
        .await?
        .ok_or(AppError::NotFound("thing"))?;
    // Links are read in the thing's own workspace's modules.
    let viewer = match thing.project_id {
        Some(project_id) => things::Viewer {
            modules: state::for_project(&mut connection, project_id, principal.actor_id)
                .await?
                .unwrap_or_default(),
            ..viewer
        },
        None => viewer,
    };
    let links = things::links(&mut connection, &viewer, &thing).await?;
    Ok(Json(ThingResponse { thing, links }))
}

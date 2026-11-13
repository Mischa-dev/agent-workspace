//! Server routes of a module that's off answer "turned off" (AWD-97).
//!
//! The app, agents' tools, search and the rest already hide an off module.
//! This layer closes the HTTP side: a route the module's manifest lists,
//! called in a workspace where that module is off, answers a typed
//! `module_off` error naming the module instead of doing anything.
//!
//! The workspace comes from the route's decoded parameter: `/api/v1/projects/{id}/…` is the
//! project's workspace, `/api/v1/organizations/{id}/…` the workspace
//! itself. Routes that name neither (a person's inbox across workspaces, a
//! link by code) aren't checked here: their handlers already leave out what
//! is off where it lives. Roles narrow modules for the people who hold them
//! elsewhere; this follows the workspace's own switches.
//!
//! Only a signed-in caller who can see the project or workspace learns
//! that a module is off there; anyone else gets the handler's own answer.
//!
//! Each request costs no database round trip in the common case: a
//! project's workspace never changes, so it's kept once looked up, and a
//! workspace's switches are kept until a switch is written (`forget`) or
//! for `SWITCHES_TTL` at most, which covers a write whose transaction
//! commits after a request read the old switches.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use axum::{
    RequestExt,
    extract::{MatchedPath, RawPathParams, Request, State},
    http::{HeaderMap, Method},
    middleware::Next,
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use super::{
    manifest::{self, Manifest},
    state::{self, ModuleSet},
};
use crate::{AppState, auth, error::AppError};

/// The longest a workspace's switches are kept without being written.
const SWITCHES_TTL: Duration = Duration::from_secs(10);
/// Projects kept before the map starts over; each entry is small.
const MAX_PROJECTS: usize = 50_000;

/// A project's workspace, and whether it's the hidden project DMs live in.
static PROJECTS: LazyLock<Mutex<HashMap<Uuid, (Uuid, bool)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// A workspace's modules on, and when they were read.
static SWITCHES: LazyLock<Mutex<HashMap<Uuid, (ModuleSet, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Drops a workspace's kept switches; called whenever one is written.
pub fn forget(organization_id: Uuid) {
    if let Ok(mut switches) = SWITCHES.lock() {
        switches.remove(&organization_id);
    }
}

/// Where a request acts, read from its path.
#[derive(Debug, PartialEq, Eq)]
enum Scope {
    Project(Uuid),
    Workspace(Uuid),
}

/// The project or workspace a route acts in, when its template takes one
/// right after `/api/v1/projects/` or `/api/v1/organizations/`. The ID is
/// the route's decoded parameter, as its handler sees it, so an encoded
/// spelling of the same ID can't step around the gate.
fn scope<'a>(template: &str, param: impl Fn(&str) -> Option<&'a str>) -> Option<Scope> {
    let mut pattern = template.trim_start_matches('/').split('/');
    if pattern.next()? != "api" || pattern.next()? != "v1" {
        return None;
    }
    let kind = pattern.next()?;
    let name = pattern.next()?.strip_prefix('{')?.strip_suffix('}')?;
    let id = Uuid::parse_str(param(name)?).ok()?;
    match kind {
        "projects" => Some(Scope::Project(id)),
        "organizations" => Some(Scope::Workspace(id)),
        _ => None,
    }
}

/// Whether the module is off where the request acts.
async fn off_module(
    state: &AppState,
    module: &'static Manifest,
    scope: &Scope,
) -> Result<bool, AppError> {
    let organization_id = match scope {
        Scope::Workspace(id) => *id,
        Scope::Project(project_id) => {
            let project_id = *project_id;
            let known = PROJECTS
                .lock()
                .ok()
                .and_then(|projects| projects.get(&project_id).copied());
            let (organization_id, direct) = match known {
                Some(found) => found,
                None => {
                    let Some(found) = sqlx::query_as::<_, (Uuid, bool)>(
                        "SELECT organization_id, kind = 'direct' FROM projects WHERE id = $1",
                    )
                    .bind(project_id)
                    .fetch_optional(&state.pool)
                    .await?
                    else {
                        // No such project: the handler answers that.
                        return Ok(false);
                    };
                    if let Ok(mut projects) = PROJECTS.lock() {
                        if projects.len() >= MAX_PROJECTS {
                            projects.clear();
                        }
                        projects.insert(project_id, found);
                    }
                    found
                }
            };
            // DMs live in a hidden project and follow the Messages switch,
            // which their own handlers check.
            if direct {
                return Ok(false);
            }
            organization_id
        }
    };
    let kept = SWITCHES.lock().ok().and_then(|switches| {
        switches
            .get(&organization_id)
            .filter(|(_, read)| read.elapsed() < SWITCHES_TTL)
            .map(|(set, _)| *set)
    });
    let set = match kept {
        Some(set) => set,
        None => {
            let mut connection = state.pool.acquire().await?;
            let set = state::workspace(&mut connection, organization_id).await?;
            if let Ok(mut switches) = SWITCHES.lock() {
                switches.insert(organization_id, (set, Instant::now()));
            }
            set
        }
    };
    Ok(!set.contains(&module.id))
}

/// Whether the signed-in caller may see where the request acts. Only they
/// learn that a module is off there: anyone else gets the handler's own
/// answer (sign in, or not found), as if the gate weren't there.
async fn caller_can_see(
    state: &AppState,
    headers: &HeaderMap,
    scope: &Scope,
) -> Result<bool, AppError> {
    let Some(actor) = auth::load_authenticated_actor(state, headers).await? else {
        return Ok(false);
    };
    let visible = match scope {
        Scope::Project(project_id) => {
            sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, NULL, $2)")
                .bind(actor.actor_id)
                .bind(project_id)
                .fetch_one(&state.pool)
                .await?
        }
        Scope::Workspace(organization_id) => {
            sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM organization_memberships membership
                    JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
                    WHERE membership.organization_id = $1 AND membership.actor_id = $2
                      AND membership.enabled
                )
                "#,
            )
            .bind(organization_id)
            .bind(actor.actor_id)
            .fetch_one(&state.pool)
            .await?
        }
    };
    Ok(visible)
}

/// Routes that answer whatever is off, because they say what is: opening
/// a project lists the modules off there (`modulesOff`), which is how the
/// app knows what to hide.
const ALWAYS_ANSWER: [&str; 1] = ["/api/v1/projects/{id}"];

/// The route layer: refuses a route whose module is off where it acts.
pub async fn refuse_off_modules(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let params = request.extract_parts::<RawPathParams>().await.ok();
    let module = request
        .extensions()
        .get::<MatchedPath>()
        .and_then(|matched| {
            if ALWAYS_ANSWER.contains(&matched.as_str()) && request.method() == Method::GET {
                return None;
            }
            let module = manifest::of_route(matched.as_str())?;
            let scope = scope(matched.as_str(), |name| {
                params
                    .as_ref()?
                    .iter()
                    .find_map(|(key, value)| (key == name).then_some(value))
            })?;
            module.switchable.then_some((module, scope))
        });
    if let Some((module, scope)) = module {
        // Who's asking only matters once something is off, so the common
        // case stays free of the session lookup.
        let refused = match off_module(&state, module, &scope).await {
            Ok(true) => caller_can_see(&state, request.headers(), &scope).await,
            other => other,
        };
        match refused {
            Ok(false) => {}
            Ok(true) => {
                return AppError::ModuleOff {
                    module: module.id.as_str(),
                }
                .into_response();
            }
            Err(error) => return error.into_response(),
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_is_the_project_or_workspace_the_route_names() {
        const ID: &str = "01a1ad5f-a946-7cf1-8485-31786e1a1afd";
        let parsed = Uuid::from_u128(0x01a1ad5f_a946_7cf1_8485_31786e1a1afd);
        let only = |wanted: &'static str, value: &'static str| {
            move |name: &str| (name == wanted).then_some(value)
        };
        let id = ID;
        assert_eq!(
            scope(
                "/api/v1/projects/{project_id}/work-items",
                only("project_id", id)
            ),
            Some(Scope::Project(parsed))
        );
        assert_eq!(
            scope(
                "/api/v1/organizations/{organization_id}/ai",
                only("organization_id", id)
            ),
            Some(Scope::Workspace(parsed))
        );
        // A fixed segment, another kind, or something that isn't an ID.
        assert_eq!(scope("/api/v1/projects/mine", only("id", id)), None);
        assert_eq!(
            scope("/api/v1/conversations/{id}/threads", only("id", id)),
            None
        );
        assert_eq!(
            scope("/api/v1/projects/{id}", only("id", "not-an-id")),
            None
        );
        assert_eq!(scope("/connect/agent/{code}", only("code", id)), None);
    }
}

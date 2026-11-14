//! Server routes of a module that's off answer "turned off" (AWD-97).
//!
//! The app, agents' tools, search and the rest already hide an off module.
//! This layer closes the HTTP side: a route the module's manifest lists,
//! called in a workspace where that module is off, answers a typed
//! `module_off` error naming the module instead of doing anything.
//!
//! The workspace comes from the route's decoded parameter: `/api/v1/projects/{id}/…` is the
//! project's workspace, `/api/v1/organizations/{id}/…` the workspace
//! itself, and a page or whiteboard named by id (`ID_FAMILIES`, P0.7) is
//! looked up to its project or workspace. Routes that name none of these (a
//! person's inbox across workspaces, a link by code, a conversation by id)
//! aren't checked here (`ID_EXEMPT` says why for each id-routed family):
//! their handlers already leave out what is off where it lives.
//!
//! A role's modules off are a permission (P0 question 1, assumed): when any
//! role in the workspace turns a module off, the gate also narrows by the
//! caller's own role there, as MCP does.
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
/// Workspaces whose switches are kept at once.
const MAX_WORKSPACES: usize = 10_000;

/// A project's workspace, and its kind (`standard`, `direct` or `workspace`).
static PROJECTS: LazyLock<Mutex<HashMap<Uuid, (Uuid, &'static str)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// A workspace's modules on, whether any role there turns modules off, and
/// when they were read.
static SWITCHES: LazyLock<Mutex<HashMap<Uuid, (ModuleSet, bool, Instant)>>> =
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
fn scope(template: &str, param: impl Fn(&str) -> Option<String>) -> Option<Scope> {
    let mut pattern = template.trim_start_matches('/').split('/');
    if pattern.next()? != "api" || pattern.next()? != "v1" {
        return None;
    }
    let kind = pattern.next()?;
    let name = pattern.next()?.strip_prefix('{')?.strip_suffix('}')?;
    let id = Uuid::parse_str(&param(name)?).ok()?;
    match kind {
        "projects" => Some(Scope::Project(id)),
        "organizations" => Some(Scope::Workspace(id)),
        _ => None,
    }
}

/// Families of routes that name content by id rather than by project:
/// (path prefix, the id's parameter, the lookup returning its project and
/// workspace). An id that resolves to nothing passes to the handler, as an
/// unknown project does.
pub(crate) const ID_FAMILIES: &[(&str, &str, &str)] = &[
    (
        "/api/v1/pages/",
        "id",
        "SELECT page.project_id, project.organization_id FROM project_pages page \
         JOIN projects project ON project.id = page.project_id WHERE page.id = $1",
    ),
    (
        "/api/v1/whiteboards/",
        "id",
        "SELECT project_id, organization_id FROM whiteboards WHERE id = $1",
    ),
];

/// Id-routed families the gate doesn't look up, and why. Every switchable
/// route named by id is in `ID_FAMILIES` or here (a test in `modules`).
pub(crate) const ID_EXEMPT: &[(&str, &str)] = &[
    (
        "/api/v1/agent-approvals/",
        "an approval belongs to a run; the run's thread is checked by conversation access",
    ),
    (
        "/api/v1/agent-runs/",
        "runs are read through their thread, whose handler checks the Runs module",
    ),
    (
        "/api/v1/bug-report-links/",
        "a link is managed from its project's settings, which the gate covers",
    ),
    (
        "/api/v1/bug-reports/",
        "a public report link by token, posted without a session; the gate answers only signed-in callers",
    ),
    (
        "/api/v1/channels/",
        "conversations follow the Messages switch, checked by their handlers",
    ),
    (
        "/api/v1/conversations/",
        "conversations follow the Messages switch, checked by their handlers",
    ),
    (
        "/api/v1/files/",
        "a file is read with the thing it is attached to",
    ),
    (
        "/api/v1/forms/",
        "a public form by token; its collection's module is checked by the handler",
    ),
    (
        "/api/v1/later/",
        "a person's own saved items, across workspaces",
    ),
    (
        "/api/v1/messages/",
        "a message is read with its conversation",
    ),
    (
        "/api/v1/project-invitations/",
        "an invitation is revoked or replaced from its project's members screen, whose routes the gate covers",
    ),
    (
        "/api/v1/questions/",
        "questions belong to a conversation, checked by its handler",
    ),
    (
        "/api/v1/reply-requests/",
        "reply requests belong to a conversation, checked by its handler",
    ),
    (
        "/api/v1/scheduled-messages/",
        "a person's own scheduled messages, checked with their conversation",
    ),
    (
        "/api/v1/threads/",
        "conversations follow the Messages switch, checked by their handlers",
    ),
    (
        "/api/v1/webhooks/",
        "webhooks are workspace settings, checked by their handler",
    ),
];

/// The scope a route named by id acts in, looked up from the database.
async fn id_scope(
    state: &AppState,
    template: &str,
    param: impl Fn(&str) -> Option<String>,
) -> Result<Option<Scope>, AppError> {
    let Some((_, name, lookup)) = ID_FAMILIES.iter().find(|(prefix, name, _)| {
        template
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(&format!("{{{name}}}")))
    }) else {
        return Ok(None);
    };
    let Some(id) = param(name).and_then(|raw| Uuid::parse_str(&raw).ok()) else {
        return Ok(None);
    };
    let found = sqlx::query_as::<_, (Option<Uuid>, Uuid)>(lookup)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    Ok(found.map(|(project, workspace)| match project {
        Some(project) => Scope::Project(project),
        None => Scope::Workspace(workspace),
    }))
}

/// Routes of the Projects module that pages in the workspace space still
/// use, though only Pages is on there (AWD-96): what the person sees there,
/// where they were, and who they can mention.
const SPACE_PLUMBING: [(&str, Option<Method>); 3] = [
    ("/api/v1/projects/{project_id}/layout", Some(Method::GET)),
    ("/api/v1/projects/{id}/navigation", None),
    ("/api/v1/projects/{id}/mentionable-actors", None),
];

fn space_plumbing(template: &str, method: &Method) -> bool {
    SPACE_PLUMBING
        .iter()
        .any(|(route, only)| *route == template && only.as_ref().is_none_or(|only| only == method))
}

/// The module that's off where the request acts, if any: the route's own,
/// or Projects for anything in a project while Projects is off.
/// What the workspace's switches say about a request.
#[derive(Debug, PartialEq, Eq)]
enum Off {
    /// On, and no role in the workspace narrows modules.
    No,
    /// Off for everyone there.
    Module(&'static str),
    /// On, but some role there turns modules off: ask the caller's role.
    ForRoles,
}

async fn off_module(
    state: &AppState,
    module: &'static Manifest,
    scope: &Scope,
    plumbing: bool,
) -> Result<Off, AppError> {
    let (organization_id, kind) = match scope {
        Scope::Workspace(id) => (*id, None),
        Scope::Project(project_id) => {
            let project_id = *project_id;
            let known = PROJECTS
                .lock()
                .ok()
                .and_then(|projects| projects.get(&project_id).copied());
            let (organization_id, kind) = match known {
                Some(found) => found,
                None => {
                    let Some((organization_id, kind)) = sqlx::query_as::<_, (Uuid, String)>(
                        "SELECT organization_id, kind FROM projects WHERE id = $1",
                    )
                    .bind(project_id)
                    .fetch_optional(&state.pool)
                    .await?
                    else {
                        // No such project: the handler answers that.
                        return Ok(Off::No);
                    };
                    let kind = match kind.as_str() {
                        "direct" => "direct",
                        "workspace" => "workspace",
                        _ => "standard",
                    };
                    if let Ok(mut projects) = PROJECTS.lock() {
                        if projects.len() >= MAX_PROJECTS {
                            projects.clear();
                        }
                        projects.insert(project_id, (organization_id, kind));
                    }
                    (organization_id, kind)
                }
            };
            // DMs live in a hidden project and follow the Messages switch,
            // which their own handlers check.
            if kind == "direct" {
                return Ok(Off::No);
            }
            if kind == "workspace" && plumbing {
                return Ok(Off::No);
            }
            (organization_id, Some(kind))
        }
    };
    let kept = SWITCHES.lock().ok().and_then(|switches| {
        switches
            .get(&organization_id)
            .filter(|(_, _, read)| read.elapsed() < SWITCHES_TTL)
            .map(|(set, narrowing, _)| (*set, *narrowing))
    });
    let (set, narrowing) = match kept {
        Some(kept) => kept,
        None => {
            let mut connection = state.pool.acquire().await?;
            let set = state::workspace(&mut connection, organization_id).await?;
            let narrowing = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM workspace_roles WHERE organization_id = $1 AND cardinality(modules_off) > 0)",
            )
            .bind(organization_id)
            .fetch_one(&mut *connection)
            .await?;
            if let Ok(mut switches) = SWITCHES.lock() {
                // IDs come from the path, so anyone can name new ones:
                // drop expired entries once full, and start over if that
                // isn't enough.
                if switches.len() >= MAX_WORKSPACES {
                    switches.retain(|_, (_, _, read)| read.elapsed() < SWITCHES_TTL);
                    if switches.len() >= MAX_WORKSPACES {
                        switches.clear();
                    }
                }
                switches.insert(organization_id, (set, narrowing, Instant::now()));
            }
            (set, narrowing)
        }
    };
    // AWD-96: anything in a project needs Projects; the workspace space
    // holds only pages.
    if kind == Some("standard") && !set.contains("projects") && module.id != "projects" {
        return Ok(Off::Module("projects"));
    }
    let here = kind.map_or(set, |kind| set.in_project(kind));
    if !here.contains(&module.id) {
        return Ok(Off::Module(module.id.as_str()));
    }
    Ok(if narrowing { Off::ForRoles } else { Off::No })
}

/// The module that's off for the signed-in caller where the request acts,
/// narrowed by their role there; none for anyone who isn't a member (the
/// handler answers them).
async fn off_for_caller(
    state: &AppState,
    headers: &HeaderMap,
    module: &'static Manifest,
    scope: &Scope,
) -> Result<Option<&'static str>, AppError> {
    let Some(actor) = auth::load_authenticated_actor(state, headers).await? else {
        return Ok(None);
    };
    let mut connection = state.pool.acquire().await?;
    let set = match scope {
        Scope::Project(project_id) => {
            if !sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, NULL, $2)")
                .bind(actor.actor_id)
                .bind(project_id)
                .fetch_one(&mut *connection)
                .await?
            {
                return Ok(None);
            }
            let Some(set) =
                state::for_project(&mut connection, *project_id, actor.actor_id).await?
            else {
                return Ok(None);
            };
            set
        }
        Scope::Workspace(organization_id) => {
            if !caller_can_see(state, headers, scope).await? {
                return Ok(None);
            }
            state::for_member(&mut connection, *organization_id, actor.actor_id).await?
        }
    };
    Ok((!set.contains(&module.id)).then_some(module.id.as_str()))
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
    let param = |name: &str| {
        params
            .as_ref()?
            .iter()
            .find_map(|(key, value)| (key == name).then(|| value.to_owned()))
    };
    let matched = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned());
    let module = matched.as_deref().and_then(|template| {
        if ALWAYS_ANSWER.contains(&template) && request.method() == Method::GET {
            return None;
        }
        let module = manifest::of_route(template)?;
        let plumbing = space_plumbing(template, request.method());
        module.switchable.then_some((module, plumbing))
    });
    let module = match (module, matched.as_deref()) {
        (Some((module, plumbing)), Some(template)) => {
            let found = match scope(template, |name| param(name)) {
                Some(found) => Ok(Some(found)),
                None => id_scope(&state, template, |name| param(name)).await,
            };
            match found {
                Ok(Some(found)) => Some((module, found, plumbing)),
                Ok(None) => None,
                Err(error) => return error.into_response(),
            }
        }
        _ => None,
    };
    if let Some((module, scope, plumbing)) = module {
        // Who's asking only matters once something is off (or a role in the
        // workspace narrows modules), so the common case stays free of the
        // session lookup.
        let refused = match off_module(&state, module, &scope, plumbing).await {
            Ok(Off::Module(off)) => caller_can_see(&state, request.headers(), &scope)
                .await
                .map(|seen| seen.then_some(off)),
            Ok(Off::No) => Ok(None),
            Ok(Off::ForRoles) => off_for_caller(&state, request.headers(), module, &scope).await,
            Err(error) => Err(error),
        };
        match refused {
            Ok(None) => {}
            Ok(Some(off)) => return AppError::ModuleOff { module: off }.into_response(),
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
            move |name: &str| (name == wanted).then(|| value.to_owned())
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

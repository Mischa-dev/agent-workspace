//! `GET /api/v1/links/resolve?path=...` (K4.18, K4 §3.10 decision 7): an
//! address someone already has (today's `/projects/<id>/work-items/<id>`, a
//! DM's `/messages/<id>`, a workspace address with a retired slug or an id)
//! as the address it has now, in its workspace:
//! `{"path": "/w/acme/s/<id>/tasks/<id>", "workspaceId": "..."}`.
//!
//! It answers only when the caller may see what the address leads to; an
//! unknown address and one they may not see both answer 404, so a stranger
//! learns nothing, not even a workspace's slug. The server never redirects
//! old paths itself (that would need the session to pick a workspace and
//! would show the slug): the web's catch-all for old paths calls this.

use axum::{Json, Router, extract::State, routing::get};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{WorkspaceAddress, split_path, workspace_path};
use crate::{
    AppState,
    access::{self, Need},
    auth::AuthenticatedActor,
    error::{ApiQuery, AppError},
    modules::{state, things},
};

/// The longest address resolve reads.
const MAX_PATH: usize = 2048;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/links/resolve", get(resolve_http))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveQuery {
    path: String,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    /// The address now, with the workspace in it and the query and hash
    /// kept.
    pub path: String,
    pub workspace_id: Uuid,
}

async fn resolve_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<ResolveQuery>,
) -> Result<Json<Resolved>, AppError> {
    let caller = principal.caller()?;
    let projects = crate::search::browser_projects(&state.pool, principal.actor_id).await?;
    let task_projects =
        crate::search::projects_showing_tasks(&state.pool, principal.actor_id, &projects).await?;
    let mut transaction = crate::db::scoped_snapshot(&state.pool).await?;
    let connection = &mut *transaction;
    let modules = state::anywhere(connection, principal.actor_id).await?;
    let workspaces = state::by_workspace(connection, principal.actor_id).await?;
    let viewer = things::Viewer {
        actor_id: principal.actor_id,
        projects: &projects,
        task_projects: &task_projects,
        modules,
        workspaces: &workspaces,
        addresses: super::Addresses::of(&state),
    };
    let Some((workspace, path)) = resolve(connection, &viewer, &query.path).await? else {
        return Err(AppError::NotFound("address"));
    };
    // The workspace the address leads into must be one the caller is in.
    if access::check(
        connection,
        &caller,
        Need::SEE,
        access::Target::Workspace(workspace.id),
    )
    .await?
    .is_err()
    {
        return Err(AppError::NotFound("address"));
    }
    transaction.commit().await?;
    Ok(Json(Resolved {
        path,
        workspace_id: workspace.id,
    }))
}

/// Where an address leads for `viewer`: the workspace, and the address now
/// (query and hash kept). `None` when it leads nowhere they may go. The
/// caller still checks that the viewer is in the workspace.
pub async fn resolve(
    connection: &mut PgConnection,
    viewer: &things::Viewer<'_>,
    path: &str,
) -> Result<Option<(WorkspaceAddress, String)>, AppError> {
    if path.len() > MAX_PATH || !path.starts_with('/') || path.starts_with("//") {
        return Ok(None);
    }
    let (pathname, rest) = split_path(path);
    let segments: Vec<&str> = pathname
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let id = |index: usize| segments.get(index).and_then(|s| Uuid::parse_str(s).ok());
    let workspace = match segments.as_slice() {
        // Already a workspace address: a retired slug or an id becomes the
        // current slug.
        ["w", reference, tail @ ..] => {
            return workspace_address(connection, viewer, reference, tail, rest).await;
        }
        ["messages", _] | ["dm", _] => match id(1) {
            Some(conversation) => direct_workspace(connection, viewer, conversation).await?,
            None => default_workspace(connection, viewer).await?,
        },
        ["runs", _, ..] => match id(1) {
            Some(run) => {
                crate::runs::store::workspace_of_visible(&mut *connection, viewer.actor_id, run)
                    .await?
            }
            None => None,
        },
        ["whiteboards", _, ..] => match id(1) {
            Some(board) => whiteboard_workspace(connection, viewer, board).await?,
            None => None,
        },
        ["projects", _, tail @ ..] if id(1).is_some() => {
            space_workspace(connection, viewer, id(1).expect("checked"), tail).await?
        }
        ["settings", "projects", _, ..] => match id(2) {
            Some(space) => space_workspace(connection, viewer, space, &[]).await?,
            None => None,
        },
        ["settings", "workspace", ..] => default_workspace(connection, viewer).await?,
        _ => {
            // A workspace's own page at the root (`/inbox`, `/projects`):
            // the workspace the caller used last.
            let legacy = format!("{pathname}{rest}");
            if workspace_path("-", &legacy).is_none() {
                return Ok(None);
            }
            default_workspace(connection, viewer).await?
        }
    };
    let Some(workspace) = workspace else {
        return Ok(None);
    };
    let address = super::address(connection, workspace).await?;
    Ok(
        workspace_path(&address.segment(), &format!("{pathname}{rest}"))
            .map(|path| (address, path)),
    )
}

/// `/w/<slug or id>/...`: the workspace by its current slug, a retired one or
/// its id. A space named inside (`/s/<id>`) must be in that workspace and
/// visible to the caller.
async fn workspace_address(
    connection: &mut PgConnection,
    viewer: &things::Viewer<'_>,
    reference: &str,
    tail: &[&str],
    rest: &str,
) -> Result<Option<(WorkspaceAddress, String)>, AppError> {
    let workspace = match Uuid::parse_str(reference) {
        Ok(id) => Some(id),
        Err(_) => {
            let slug = reference.to_ascii_lowercase();
            // visibility: the workspace's membership is checked by the route.
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM organizations WHERE slug = $1 \
                 UNION ALL SELECT workspace_id FROM workspace_slugs WHERE slug = $1 \
                 LIMIT 1",
            )
            .bind(&slug)
            .fetch_optional(&mut *connection)
            .await?
        }
    };
    let Some(workspace) = workspace else {
        return Ok(None);
    };
    match tail {
        ["s", space, ..] => {
            let Ok(space) = Uuid::parse_str(space) else {
                return Ok(None);
            };
            if space_workspace(connection, viewer, space, &[]).await? != Some(workspace) {
                return Ok(None);
            }
        }
        ["dm", conversation, ..] => {
            let Ok(conversation) = Uuid::parse_str(conversation) else {
                return Ok(None);
            };
            if direct_workspace(connection, viewer, conversation).await? != Some(workspace) {
                return Ok(None);
            }
        }
        _ => {}
    }
    let Ok(address) = super::address(connection, workspace).await else {
        return Ok(None);
    };
    let inner: String = tail.iter().map(|segment| format!("/{segment}")).collect();
    let path = format!("/w/{}{inner}{rest}", address.segment());
    Ok(Some((address, path)))
}

/// The workspace the caller used last (or joined first).
async fn default_workspace(
    connection: &mut PgConnection,
    viewer: &things::Viewer<'_>,
) -> Result<Option<Uuid>, AppError> {
    Ok(
        crate::workspace::default_workspace(&mut *connection, viewer.actor_id)
            .await?
            .map(|(workspace, _)| workspace),
    )
}

/// The workspace of a DM or group the viewer is in.
async fn direct_workspace(
    connection: &mut PgConnection,
    viewer: &things::Viewer<'_>,
    conversation: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let Some(thing) = things::get(connection, viewer, "conversation", conversation).await? else {
        return Ok(None);
    };
    if thing.kind != "conversation" {
        // A channel or thread lives in its space, not with Messages.
        return Ok(None);
    }
    space_of(connection, thing.project_id).await
}

/// The workspace of a whiteboard the viewer can see.
async fn whiteboard_workspace(
    connection: &mut PgConnection,
    viewer: &things::Viewer<'_>,
    board: Uuid,
) -> Result<Option<Uuid>, AppError> {
    if things::get(connection, viewer, "whiteboard", board)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    // visibility: checked through things::get just above.
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT COALESCE(board.organization_id, project.organization_id) \
         FROM whiteboards board LEFT JOIN projects project ON project.id = board.project_id \
         WHERE board.id = $1",
    )
    .bind(board)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The workspace of a space (project) and the place named in `tail`, when
/// the viewer can see them: a task, page, channel or thread through its
/// thing (it must be in that space), a collection or sheet by its row,
/// anything else through the space itself.
async fn space_workspace(
    connection: &mut PgConnection,
    viewer: &things::Viewer<'_>,
    space: Uuid,
    tail: &[&str],
) -> Result<Option<Uuid>, AppError> {
    let child = tail
        .get(1)
        .and_then(|segment| Uuid::parse_str(segment).ok());
    let kind = match (tail.first().copied(), child) {
        (Some("work-items"), Some(_)) => Some("task"),
        (Some("pages"), Some(_)) => Some("page"),
        (Some("channels"), Some(_)) => Some("channel"),
        (Some("threads"), Some(_)) => Some("thread"),
        _ => None,
    };
    let visible = match (kind, child) {
        (Some(kind), Some(child)) => things::get(connection, viewer, kind, child)
            .await?
            .is_some_and(|thing| thing.project_id == Some(space)),
        _ => match (tail.first().copied(), child) {
            (Some("collections"), Some(child)) => {
                viewer.projects.contains(&space)
                    && exists_in_space(connection, "collections", space, child).await?
            }
            (Some("sheets"), Some(child)) => {
                viewer.projects.contains(&space)
                    && exists_in_space(connection, "sheets", space, child).await?
            }
            _ => things::get(connection, viewer, "project", space)
                .await?
                .is_some(),
        },
    };
    if !visible {
        return Ok(None);
    }
    space_of(connection, Some(space)).await
}

/// Whether a collection or sheet `id` is in `space` (and not archived).
async fn exists_in_space(
    connection: &mut PgConnection,
    table: &'static str,
    space: Uuid,
    id: Uuid,
) -> Result<bool, AppError> {
    let sql = match table {
        "collections" => {
            "SELECT EXISTS (SELECT 1 FROM collections WHERE project_id = $1 AND id = $2 \
             AND archived_at IS NULL)"
        }
        _ => "SELECT EXISTS (SELECT 1 FROM sheets WHERE project_id = $1 AND id = $2)",
    };
    Ok(sqlx::query_scalar::<_, bool>(sql)
        .bind(space)
        .bind(id)
        .fetch_one(&mut *connection)
        .await?)
}

/// The workspace a space is in.
async fn space_of(
    connection: &mut PgConnection,
    space: Option<Uuid>,
) -> Result<Option<Uuid>, AppError> {
    let Some(space) = space else {
        return Ok(None);
    };
    Ok(
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(space)
            .fetch_optional(&mut *connection)
            .await?,
    )
}

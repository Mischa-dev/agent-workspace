//! Whiteboards: Excalidraw boards that belong to a workspace, which people
//! draw on in the browser and agents edit through MCP, with no browser open.
//!
//! The server owns each board's scene (see `store`): browsers send the
//! elements they changed and fetch what changed since their last look, and
//! agents' operations (see `scene`) write through the same path, so both
//! merge by Excalidraw's own per-element version rule. A board without a
//! project is for everyone in its workspace; a board in a project is for
//! that project's members. Links put one board on any number of tasks,
//! projects and pages without copying it.
//!
//! Every read checks `whiteboard_visible_to` (migration 20261017070000) and,
//! for agents, `actor_can_access_project` on the board's project.

pub mod edit;
pub mod elements;
pub mod fractional;
mod http;
pub mod render;
pub mod scene;
pub mod store;

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

pub use http::router;

use crate::{
    auth::AuthenticatedActor,
    conversations::parse_actor_kind,
    error::AppError,
    memory,
    policy::{Action, ActorKind},
};

pub const MAX_TITLE: usize = 200;

/// Who is acting on a board, and how far their connection reaches.
#[derive(Clone, Copy, Debug)]
pub struct Caller {
    pub actor_id: Uuid,
    pub actor_kind: ActorKind,
    /// The connected app a person acts through, recorded on revisions.
    pub via_grant_id: Option<Uuid>,
    /// An agent's connection, whose rules decide the projects it reaches.
    pub connection_id: Option<Uuid>,
    /// An older project-scoped credential that stays in one project.
    pub confined_project: Option<Uuid>,
    /// Where the call came from, for the audit log: browser or mcp.
    pub channel: &'static str,
    pub authentication_class: &'static str,
    pub session_id: Option<Uuid>,
}

impl Caller {
    pub fn browser(principal: &AuthenticatedActor) -> Result<Self, AppError> {
        Ok(Self {
            actor_id: principal.actor_id,
            actor_kind: parse_actor_kind(&principal.actor_kind)?,
            via_grant_id: None,
            connection_id: None,
            confined_project: None,
            channel: "browser",
            authentication_class: "password_session",
            session_id: Some(principal.session_id),
        })
    }
}

/// A board as lists and pages show it.
#[derive(Clone, Debug, FromRow, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoardSummary {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub organization_id: Uuid,
    pub organization_name: String,
    /// The project whose members see it; empty for the whole workspace.
    #[schemars(with = "Option<String>")]
    pub project_id: Option<Uuid>,
    pub project_name: Option<String>,
    pub title: String,
    /// The scene's change counter: it goes up with every change.
    pub scene_seq: i64,
    /// The board's own version (title, who sees it, trash).
    pub version: i64,
    pub element_count: i32,
    pub created_at: String,
    pub created_by: Option<String>,
    pub updated_at: String,
    pub scene_updated_at: String,
    pub updated_by: Option<String>,
    /// human or external_agent: who changed it last.
    pub updated_by_kind: Option<String>,
    pub deleted: bool,
    pub link_count: i64,
}

const SUMMARY_SELECT: &str = r#"
    SELECT board.id, board.organization_id, organization.name AS organization_name,
           board.project_id, project.name AS project_name, board.title, board.scene_seq,
           board.version, board.element_count, board.created_at::text AS created_at,
           (SELECT COALESCE(profile.display_name, account.display_name)
              FROM actors author
              LEFT JOIN organization_actor_profiles profile
                ON profile.organization_id = board.organization_id AND profile.actor_id = author.id
              LEFT JOIN users account ON account.actor_id = author.id
             WHERE author.id = board.created_by_actor_id) AS created_by,
           board.updated_at::text AS updated_at,
           board.scene_updated_at::text AS scene_updated_at,
           (SELECT COALESCE(profile.display_name, account.display_name)
              FROM actors editor
              LEFT JOIN organization_actor_profiles profile
                ON profile.organization_id = board.organization_id AND profile.actor_id = editor.id
              LEFT JOIN users account ON account.actor_id = editor.id
             WHERE editor.id = board.updated_by_actor_id) AS updated_by,
           (SELECT editor.kind FROM actors editor WHERE editor.id = board.updated_by_actor_id)
             AS updated_by_kind,
           board.deleted_at IS NOT NULL AS deleted,
           (SELECT count(*) FROM whiteboard_links link WHERE link.whiteboard_id = board.id)
             AS link_count
    FROM whiteboards board
    JOIN organizations organization ON organization.id = board.organization_id
    LEFT JOIN projects project ON project.id = board.project_id
"#;

/// What lists may filter on.
#[derive(Clone, Debug, Default)]
pub struct ListFilter {
    pub organization_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub query: Option<String>,
    pub deleted: bool,
    pub limit: i64,
    /// AWD-75: when set, only boards in these projects, or across these
    /// workspaces, before the limit (where Whiteboards is on for the caller).
    pub on_in: Option<OnIn>,
}

/// The projects, and the workspaces for boards across one, where a module is on.
#[derive(Clone, Debug, Default)]
pub struct OnIn {
    pub projects: Vec<Uuid>,
    pub workspaces: Vec<Uuid>,
}

/// Boards the caller can see, most recently changed first.
pub async fn list(
    pool: &PgPool,
    caller: &Caller,
    filter: &ListFilter,
) -> Result<Vec<BoardSummary>, AppError> {
    let query = filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(|query| query.chars().take(200).collect::<String>());
    Ok(sqlx::query_as::<_, BoardSummary>(&format!(
        r#"{SUMMARY_SELECT}
        WHERE whiteboard_visible_to(board.id, $1)
          AND (board.project_id IS NULL OR actor_can_access_project($1, $2, board.project_id))
          AND ($3::uuid IS NULL OR board.project_id = $3)
          AND ($4::uuid IS NULL OR board.organization_id = $4)
          AND ($5::uuid IS NULL OR board.project_id = $5)
          AND ((board.deleted_at IS NOT NULL) = $6)
          AND (NOT $9 OR (board.project_id IS NOT NULL AND board.project_id = ANY($10))
               OR (board.project_id IS NULL AND board.organization_id = ANY($11)))
          AND ($7::text IS NULL
               OR board.search_vector @@ plainto_tsquery('simple', $7)
               OR board.title ILIKE '%' || replace(replace(replace($7, '\', '\\'), '%', '\%'), '_', '\_') || '%')
        ORDER BY board.updated_at DESC, board.id
        LIMIT $8
        "#
    ))
    .bind(caller.actor_id)
    .bind(caller.connection_id)
    .bind(caller.confined_project)
    .bind(filter.organization_id)
    .bind(filter.project_id)
    .bind(filter.deleted)
    .bind(query)
    .bind(filter.limit.clamp(1, 200))
    .bind(filter.on_in.is_some())
    .bind(filter.on_in.as_ref().map(|on| on.projects.clone()).unwrap_or_default())
    .bind(filter.on_in.as_ref().map(|on| on.workspaces.clone()).unwrap_or_default())
    .fetch_all(pool)
    .await?)
}

#[derive(Clone, Copy, Debug, FromRow)]
pub struct BoardAccess {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub project_id: Option<Uuid>,
    pub deleted: bool,
    pub can_edit: bool,
}

/// The board if the caller can see it; `need_edit` also requires that they
/// can change it. Boards someone can't see are "not found", never forbidden.
pub async fn access(
    connection: &mut sqlx::PgConnection,
    caller: &Caller,
    whiteboard_id: Uuid,
    need_edit: bool,
) -> Result<BoardAccess, AppError> {
    let row = sqlx::query_as::<_, (Uuid, Option<Uuid>, bool, bool, bool)>(
        r#"
        SELECT board.organization_id, board.project_id, board.deleted_at IS NOT NULL,
               whiteboard_visible_to(board.id, $2),
               board.project_id IS NULL OR actor_can_access_project($2, $3, board.project_id)
        FROM whiteboards board
        WHERE board.id = $1
        "#,
    )
    .bind(whiteboard_id)
    .bind(caller.actor_id)
    .bind(caller.connection_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(AppError::NotFound("whiteboard"))?;
    let (organization_id, project_id, deleted, visible, reachable) = row;
    if !visible
        || !reachable
        || caller
            .confined_project
            .is_some_and(|confined| project_id != Some(confined))
    {
        return Err(AppError::NotFound("whiteboard"));
    }
    // A workspace board is for everyone in the workspace to draw on; a
    // project's board follows the project's roles (viewers only look).
    let can_edit = match project_id {
        None => true,
        Some(project_id) => memory::project_access(
            &mut *connection,
            caller.actor_id,
            caller.actor_kind,
            project_id,
            Action::WhiteboardEdit,
        )
        .await
        .is_ok(),
    };
    if need_edit && !can_edit {
        return Err(AppError::Forbidden);
    }
    Ok(BoardAccess {
        id: whiteboard_id,
        organization_id,
        project_id,
        deleted,
        can_edit,
    })
}

/// For realtime: whether an actor may receive a board's events right now.
pub async fn visible_to(
    pool: &PgPool,
    whiteboard_id: Uuid,
    actor_id: Uuid,
) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT whiteboard_visible_to($1, $2)")
            .bind(whiteboard_id)
            .bind(actor_id)
            .fetch_one(pool)
            .await?,
    )
}

pub async fn summary(pool: &PgPool, whiteboard_id: Uuid) -> Result<BoardSummary, AppError> {
    sqlx::query_as::<_, BoardSummary>(&format!("{SUMMARY_SELECT} WHERE board.id = $1"))
        .bind(whiteboard_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("whiteboard"))
}

pub fn normalize_title(value: &str) -> Result<String, AppError> {
    let title = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() || title.chars().count() > MAX_TITLE || title.chars().any(char::is_control)
    {
        return Err(AppError::Validation("title"));
    }
    Ok(title)
}

/// Checks the caller may put a board in a project: they can edit there and
/// their connection reaches it. Returns the project's workspace.
async fn project_for_board(
    connection: &mut sqlx::PgConnection,
    caller: &Caller,
    project_id: Uuid,
) -> Result<Uuid, AppError> {
    if caller
        .confined_project
        .is_some_and(|confined| confined != project_id)
    {
        return Err(AppError::NotFound("project"));
    }
    let direct = memory::project_access(
        &mut *connection,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        Action::WhiteboardEdit,
    )
    .await?;
    let (organization_id, reachable) = sqlx::query_as::<_, (Uuid, bool)>(
        "SELECT organization_id, actor_can_access_project($2, $3, id) FROM projects WHERE id = $1",
    )
    .bind(project_id)
    .bind(caller.actor_id)
    .bind(caller.connection_id)
    .fetch_one(&mut *connection)
    .await?;
    if direct || !reachable {
        return Err(AppError::NotFound("project"));
    }
    Ok(organization_id)
}

/// Checks the caller is an active, enabled member of a workspace.
async fn require_workspace_member(
    executor: impl sqlx::PgExecutor<'_>,
    caller: &Caller,
    organization_id: Uuid,
) -> Result<(), AppError> {
    let member = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM organization_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
            WHERE membership.organization_id = $1 AND membership.actor_id = $2 AND membership.enabled
        )
        "#,
    )
    .bind(organization_id)
    .bind(caller.actor_id)
    .fetch_one(executor)
    .await?;
    if !member || caller.confined_project.is_some() {
        return Err(AppError::NotFound("organization"));
    }
    Ok(())
}

/// Where a new board goes: a project (its members see it) or a workspace
/// (everyone there does).
#[derive(Clone, Copy, Debug)]
pub enum Placement {
    Project(Uuid),
    Workspace(Uuid),
}

/// Creates an empty board. The caller commits.
pub async fn create(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    placement: Placement,
    title: &str,
) -> Result<Uuid, AppError> {
    let title = normalize_title(title)?;
    let (organization_id, project_id) = match placement {
        Placement::Project(project_id) => (
            project_for_board(transaction, caller, project_id).await?,
            Some(project_id),
        ),
        Placement::Workspace(organization_id) => {
            require_workspace_member(&mut **transaction, caller, organization_id).await?;
            (organization_id, None)
        }
    };
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO whiteboards (
            id, organization_id, project_id, title, created_by_actor_id, updated_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $5)
        "#,
    )
    .bind(id)
    .bind(organization_id)
    .bind(project_id)
    .bind(&title)
    .bind(caller.actor_id)
    .execute(&mut **transaction)
    .await?;
    audit(
        transaction,
        caller,
        organization_id,
        project_id,
        "whiteboard.created",
        id,
    )
    .await?;
    store::notify(transaction, id).await?;
    Ok(id)
}

/// Copies a board's scene and images into a new board next to it. Links stay
/// with the original.
pub async fn duplicate(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    whiteboard_id: Uuid,
    title: Option<&str>,
) -> Result<Uuid, AppError> {
    let source = access(transaction, caller, whiteboard_id, false).await?;
    let original = sqlx::query_scalar::<_, String>("SELECT title FROM whiteboards WHERE id = $1")
        .bind(whiteboard_id)
        .fetch_one(&mut **transaction)
        .await?;
    let title = match title {
        Some(title) => normalize_title(title)?,
        None => normalize_title(&format!(
            "Copy of {}",
            original.chars().take(MAX_TITLE - 8).collect::<String>()
        ))?,
    };
    let placement = match source.project_id {
        Some(project_id) => Placement::Project(project_id),
        None => Placement::Workspace(source.organization_id),
    };
    let id = create(transaction, caller, placement, &title).await?;
    sqlx::query(
        r#"
        INSERT INTO whiteboard_elements (
            whiteboard_id, element_id, element_type, version, version_nonce, is_deleted,
            sort_index, data, seq, updated_by_actor_id
        )
        SELECT $2, element_id, element_type, version, version_nonce, false, sort_index, data, 1, $3
        FROM whiteboard_elements WHERE whiteboard_id = $1 AND NOT is_deleted
        "#,
    )
    .bind(whiteboard_id)
    .bind(id)
    .bind(caller.actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO whiteboard_files (whiteboard_id, file_id, mime_type, data, created_by_actor_id)
        SELECT $2, file_id, mime_type, data, $3 FROM whiteboard_files WHERE whiteboard_id = $1
        "#,
    )
    .bind(whiteboard_id)
    .bind(id)
    .bind(caller.actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE whiteboards copy
        SET scene_seq = 1, duplicated_from_id = source.id,
            element_count = source.element_count, text_content = source.text_content,
            file_bytes = source.file_bytes
        FROM whiteboards source
        WHERE copy.id = $2 AND source.id = $1
        "#,
    )
    .bind(whiteboard_id)
    .bind(id)
    .execute(&mut **transaction)
    .await?;
    Ok(id)
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoardChange {
    pub title: Option<String>,
    /// Move the board into a project (its members see it), or with null to
    /// the whole workspace.
    #[serde(default, with = "double_option")]
    pub project_id: Option<Option<Uuid>>,
    /// Move the board to the trash (true) or bring it back (false).
    pub deleted: Option<bool>,
    /// Refuse the change if someone else changed the board since.
    pub expected_version: Option<i64>,
}

mod double_option {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

/// Renames a board, changes who sees it, or moves it to or from the trash.
/// Nothing in its scene changes, and a board in the trash keeps its links.
pub async fn change(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    whiteboard_id: Uuid,
    change: &BoardChange,
) -> Result<(), AppError> {
    let board = access(transaction, caller, whiteboard_id, true).await?;
    let (version, title, project_id, deleted) = sqlx::query_as::<_, (i64, String, Option<Uuid>, bool)>(
        "SELECT version, title, project_id, deleted_at IS NOT NULL FROM whiteboards WHERE id = $1 FOR UPDATE",
    )
    .bind(whiteboard_id)
    .fetch_one(&mut **transaction)
    .await?;
    if let Some(expected) = change.expected_version
        && expected != version
    {
        return Err(AppError::VersionConflict {
            resource: "whiteboard",
            current_version: version,
        });
    }
    let new_title = match change.title.as_deref() {
        Some(title) => normalize_title(title)?,
        None => title.clone(),
    };
    let new_project = change.project_id.unwrap_or(project_id);
    if new_project != project_id
        && let Some(target) = new_project
    {
        let organization_id = project_for_board(transaction, caller, target).await?;
        if organization_id != board.organization_id {
            return Err(AppError::NotFound("project"));
        }
    }
    let new_deleted = change.deleted.unwrap_or(deleted);
    if new_title == title && new_project == project_id && new_deleted == deleted {
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE whiteboards
        SET title = $2, project_id = $3,
            deleted_at = CASE WHEN $4 THEN COALESCE(deleted_at, now()) ELSE NULL END,
            deleted_by_actor_id = CASE WHEN $4 THEN COALESCE(deleted_by_actor_id, $5) ELSE NULL END,
            version = version + 1, updated_by_actor_id = $5, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(whiteboard_id)
    .bind(&new_title)
    .bind(new_project)
    .bind(new_deleted)
    .bind(caller.actor_id)
    .execute(&mut **transaction)
    .await?;
    let action = match (deleted, new_deleted) {
        (false, true) => "whiteboard.deleted",
        (true, false) => "whiteboard.restored",
        _ if new_project != project_id => "whiteboard.moved",
        _ => "whiteboard.renamed",
    };
    audit(
        transaction,
        caller,
        board.organization_id,
        new_project,
        action,
        whiteboard_id,
    )
    .await?;
    store::notify(transaction, whiteboard_id).await?;
    Ok(())
}

/// Deletes a board in the trash for good, with its drawing, images, saved
/// versions and links. Nothing is purged on a timer; this is the only way.
pub async fn purge(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    whiteboard_id: Uuid,
) -> Result<(), AppError> {
    let board = access(transaction, caller, whiteboard_id, true).await?;
    let deleted = sqlx::query_scalar::<_, bool>(
        "SELECT deleted_at IS NOT NULL FROM whiteboards WHERE id = $1 FOR UPDATE",
    )
    .bind(whiteboard_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !deleted {
        return Err(AppError::Conflict(
            "whiteboard",
            "move the board to the trash first",
        ));
    }
    audit(
        transaction,
        caller,
        board.organization_id,
        board.project_id,
        "whiteboard.purged",
        whiteboard_id,
    )
    .await?;
    sqlx::query("DELETE FROM whiteboards WHERE id = $1")
        .bind(whiteboard_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

/// The trash a caller sees, in the scope they asked for: `$1` the actor, `$2`
/// their connection, `$3` the project they're confined to, then the
/// workspace (`$4`) and project (`$5`) asked about.
const TRASH_IN_SCOPE: &str = r#"
    board.deleted_at IS NOT NULL
    AND whiteboard_visible_to(board.id, $1)
    AND (board.project_id IS NULL OR actor_can_access_project($1, $2, board.project_id))
    AND ($3::uuid IS NULL OR board.project_id = $3)
    AND ($4::uuid IS NULL OR board.organization_id = $4)
    AND ($5::uuid IS NULL OR board.project_id = $5)
"#;

/// Empties the trash the caller sees (one workspace or project, or all of
/// them): every board there they can change goes for good, up to 200 at a
/// time. Returns how many. With `on`, only boards where Whiteboards is on
/// for the caller (AWD-75): in those projects, or across those workspaces.
pub async fn empty_trash(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    organization_id: Option<Uuid>,
    project_id: Option<Uuid>,
    on: Option<&OnIn>,
) -> Result<usize, AppError> {
    // A project viewer sees the project's trash but can't empty it. Which
    // projects the caller can change is the policy's call, so ask it first:
    // picking 200 boards and skipping the ones they can't change would let
    // a viewer's boards fill every batch.
    let projects = sqlx::query_scalar::<_, Uuid>(&format!(
        "SELECT DISTINCT board.project_id FROM whiteboards board \
         WHERE board.project_id IS NOT NULL AND {TRASH_IN_SCOPE}"
    ))
    .bind(caller.actor_id)
    .bind(caller.connection_id)
    .bind(caller.confined_project)
    .bind(organization_id)
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    let mut editable = Vec::with_capacity(projects.len());
    for project in projects {
        if on.is_some_and(|on| !on.projects.contains(&project)) {
            continue;
        }
        if memory::project_access(
            &mut **transaction,
            caller.actor_id,
            caller.actor_kind,
            project,
            Action::WhiteboardEdit,
        )
        .await
        .is_ok()
        {
            editable.push(project);
        }
    }
    let trashed = sqlx::query_scalar::<_, Uuid>(&format!(
        r#"
        SELECT board.id FROM whiteboards board
        WHERE {TRASH_IN_SCOPE}
          AND (board.project_id IS NULL OR board.project_id = ANY($6))
          AND ($7::uuid[] IS NULL OR board.project_id IS NOT NULL
               OR board.organization_id = ANY($7))
        ORDER BY board.deleted_at, board.id
        LIMIT 200
        "#
    ))
    .bind(caller.actor_id)
    .bind(caller.connection_id)
    .bind(caller.confined_project)
    .bind(organization_id)
    .bind(project_id)
    .bind(&editable)
    .bind(on.map(|on| on.workspaces.clone()))
    .fetch_all(&mut **transaction)
    .await?;
    for whiteboard_id in &trashed {
        purge(transaction, caller, *whiteboard_id).await?;
    }
    Ok(trashed.len())
}

// ---------------------------------------------------------------------------
// Links

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Project,
    WorkItem,
    Page,
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::WorkItem => "work_item",
            Self::Page => "page",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "project" => Some(Self::Project),
            "work_item" | "task" => Some(Self::WorkItem),
            "page" => Some(Self::Page),
            _ => None,
        }
    }
}

/// The project a link target is in, and that project's workspace.
async fn target_project(
    executor: impl sqlx::PgExecutor<'_>,
    kind: TargetKind,
    target_id: Uuid,
) -> Result<(Uuid, Uuid), AppError> {
    let sql = match kind {
        TargetKind::Project => {
            "SELECT id, organization_id FROM projects WHERE id = $1 AND kind <> 'direct'"
        }
        TargetKind::WorkItem => {
            "SELECT project.id, project.organization_id FROM work_items item JOIN projects project ON project.id = item.project_id WHERE item.id = $1"
        }
        TargetKind::Page => {
            "SELECT project.id, project.organization_id FROM project_pages page JOIN projects project ON project.id = page.project_id WHERE page.id = $1"
        }
    };
    sqlx::query_as::<_, (Uuid, Uuid)>(sql)
        .bind(target_id)
        .fetch_optional(executor)
        .await?
        .ok_or(AppError::NotFound("link target"))
}

/// Puts a board on a task, project or page, or (with `linked` false) takes
/// it off. Needs edit access to the board and to the target's project, and
/// both in the same workspace. Returns whether anything changed.
pub async fn set_link(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    whiteboard_id: Uuid,
    kind: TargetKind,
    target_id: Uuid,
    linked: bool,
) -> Result<bool, AppError> {
    let board = access(transaction, caller, whiteboard_id, linked).await?;
    let (project_id, organization_id) = target_project(&mut **transaction, kind, target_id).await?;
    let reachable = caller
        .confined_project
        .is_none_or(|confined| confined == project_id)
        && sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, $2, $3)")
            .bind(caller.actor_id)
            .bind(caller.connection_id)
            .bind(project_id)
            .fetch_one(&mut **transaction)
            .await?;
    memory::project_access(
        &mut **transaction,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        Action::WhiteboardEdit,
    )
    .await
    .map_err(|_| AppError::NotFound("link target"))?;
    if !reachable || organization_id != board.organization_id {
        return Err(AppError::NotFound("link target"));
    }
    let changed = if linked {
        sqlx::query(
            r#"
            INSERT INTO whiteboard_links (
                whiteboard_id, target_kind, target_id, target_project_id, created_by_actor_id
            ) VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(whiteboard_id)
        .bind(kind.as_str())
        .bind(target_id)
        .bind(project_id)
        .bind(caller.actor_id)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "DELETE FROM whiteboard_links WHERE whiteboard_id = $1 AND target_kind = $2 AND target_id = $3",
        )
        .bind(whiteboard_id)
        .bind(kind.as_str())
        .bind(target_id)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } > 0;
    if changed {
        audit(
            transaction,
            caller,
            board.organization_id,
            Some(project_id),
            if linked {
                "whiteboard.linked"
            } else {
                "whiteboard.unlinked"
            },
            whiteboard_id,
        )
        .await?;
        sqlx::query(
            "UPDATE whiteboards SET version = version + 1, updated_at = now() WHERE id = $1",
        )
        .bind(whiteboard_id)
        .execute(&mut **transaction)
        .await?;
        store::notify(transaction, whiteboard_id).await?;
    }
    Ok(changed)
}

/// Something a board is linked to, as the viewer may see it.
#[derive(Clone, Debug, FromRow, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkView {
    /// project, work_item, or page.
    pub target_kind: String,
    #[schemars(with = "String")]
    pub target_id: Uuid,
    #[schemars(with = "String")]
    pub project_id: Uuid,
    pub project_name: String,
    pub title: String,
    /// A task's key, such as AWD-15.
    pub key: Option<String>,
    pub url: String,
}

/// A board's links, leaving out targets in projects the caller can't see.
pub async fn links(
    pool: &PgPool,
    caller: &Caller,
    whiteboard_id: Uuid,
) -> Result<Vec<LinkView>, AppError> {
    let mut links = sqlx::query_as::<_, LinkView>(
        r#"
        SELECT link.target_kind, link.target_id, link.target_project_id AS project_id,
               project.name AS project_name,
               CASE link.target_kind
                 WHEN 'project' THEN project.name
                 WHEN 'work_item' THEN COALESCE(item.title, 'Task')
                 ELSE COALESCE(page.title, 'Page')
               END AS title,
               CASE WHEN link.target_kind = 'work_item' AND item.id IS NOT NULL
                 THEN project.project_key || '-' || item.number::text END AS key,
               CASE link.target_kind
                 WHEN 'project' THEN '/projects/' || project.id::text
                 WHEN 'work_item' THEN '/projects/' || project.id::text || '/work-items/' || link.target_id::text
                 ELSE '/projects/' || project.id::text || '/pages/' || link.target_id::text
               END AS url
        FROM whiteboard_links link
        JOIN projects project ON project.id = link.target_project_id AND project.archived_at IS NULL
        LEFT JOIN work_items item ON link.target_kind = 'work_item' AND item.id = link.target_id
        LEFT JOIN project_pages page ON link.target_kind = 'page' AND page.id = link.target_id
        WHERE link.whiteboard_id = $1
          AND actor_can_access_project($2, $3, link.target_project_id)
          AND ($4::uuid IS NULL OR link.target_project_id = $4)
        ORDER BY link.created_at, link.target_id
        "#,
    )
    .bind(whiteboard_id)
    .bind(caller.actor_id)
    .bind(caller.connection_id)
    .bind(caller.confined_project)
    .fetch_all(pool)
    .await?;
    // A linked task's title is its current one: shown only with "see tasks"
    // in its project (role, and an app's agent limit), as task_open would;
    // otherwise the link keeps its key.
    let mut task_projects = links
        .iter()
        .filter(|link| link.target_kind == "work_item")
        .map(|link| link.project_id)
        .collect::<Vec<_>>();
    if !task_projects.is_empty() {
        task_projects.sort_unstable();
        task_projects.dedup();
        let showing =
            crate::search::projects_showing_tasks(pool, caller.actor_id, &task_projects).await?;
        for link in &mut links {
            if link.target_kind == "work_item" && !showing.contains(&link.project_id) {
                link.title = "Task".to_owned();
            }
        }
    }
    Ok(links)
}

/// Boards linked to a task, project or page that the caller can see. The
/// caller must be able to see the target itself.
pub async fn linked_to(
    pool: &PgPool,
    caller: &Caller,
    kind: TargetKind,
    target_id: Uuid,
) -> Result<Vec<BoardSummary>, AppError> {
    let (project_id, _) = target_project(pool, kind, target_id).await?;
    memory::project_access(
        pool,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        Action::ProjectView,
    )
    .await
    .map_err(|_| AppError::NotFound("link target"))?;
    let reachable = caller
        .confined_project
        .is_none_or(|confined| confined == project_id)
        && sqlx::query_scalar::<_, bool>("SELECT actor_can_access_project($1, $2, $3)")
            .bind(caller.actor_id)
            .bind(caller.connection_id)
            .bind(project_id)
            .fetch_one(pool)
            .await?;
    if !reachable {
        return Err(AppError::NotFound("link target"));
    }
    Ok(sqlx::query_as::<_, BoardSummary>(&format!(
        r#"{SUMMARY_SELECT}
        JOIN whiteboard_links target_link
          ON target_link.whiteboard_id = board.id
         AND target_link.target_kind = $3 AND target_link.target_id = $4
        WHERE whiteboard_visible_to(board.id, $1)
          AND (board.project_id IS NULL OR actor_can_access_project($1, $2, board.project_id))
          AND board.deleted_at IS NULL
        ORDER BY target_link.created_at, board.id
        LIMIT 100
        "#
    ))
    .bind(caller.actor_id)
    .bind(caller.connection_id)
    .bind(kind.as_str())
    .bind(target_id)
    .fetch_all(pool)
    .await?)
}

pub(crate) async fn audit(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    organization_id: Uuid,
    project_id: Option<Uuid>,
    action: &str,
    whiteboard_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, organization_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, metadata, channel,
            mcp_oauth_grant_id, external_agent_connection_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'whiteboard', $8, 'succeeded', '{}'::jsonb, $9, $10, $11)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(caller.actor_id)
    .bind(project_id)
    .bind(organization_id)
    .bind(caller.session_id)
    .bind(caller.authentication_class)
    .bind(action)
    .bind(whiteboard_id)
    .bind(caller.channel)
    .bind(caller.via_grant_id)
    .bind(caller.connection_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

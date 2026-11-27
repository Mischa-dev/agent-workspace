//! Space operations (K4.7): `space_list`, `space_open`, `space_tree_get`
//! (AWD-221), `space_personal_open` (K4.9), `space_create`,
//! `space_update`, `space_archive` and `space_restore`. The web app, agents
//! and the v2 API share them; the writes go through `spaces::manage`, which
//! writes their events.
//!
//! Reading follows the space rule (`space_readable_by`): members, and every
//! workspace member for a space open to the workspace. Making a space needs
//! "Create projects" in the workspace; changing one needs "Manage projects"
//! in it. Putting a space inside another (AWD-221), when it is made or
//! moved, needs "Manage projects" in that one too. Bringing an archived space back is checked against what its
//! members held there, since an archived space is closed to everything else.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{
    SpaceKind, SpaceVisibility,
    manage::{self, Changes, Locked, NewSpace, Owner},
};
use crate::{
    AppState,
    access::{self, Need},
    addresses::{Addresses, Place, Target as Address, WorkspaceAddress},
    error::AppError,
    ops::{
        DenyReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<SpaceList>();
    registry.add::<SpaceOpen>();
    registry.add::<SpaceTreeGet>();
    registry.add::<SpacePersonalOpen>();
    registry.add::<SpaceCreate>();
    registry.add::<SpaceUpdate>();
    registry.add::<SpaceArchive>();
    registry.add::<SpaceRestore>();
}

/// A space as its readers see it.
#[derive(Clone, Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SpaceView {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[schemars(with = "String")]
    pub workspace_id: Uuid,
    /// `workspace`, `project`, `team` or `personal`.
    #[sqlx(try_from = "String")]
    pub kind: StoredKind,
    /// Task numbers start with it, such as WEB-12.
    pub key: String,
    pub name: String,
    pub description: String,
    /// An emoji shown beside the name, when it has one.
    pub icon: Option<String>,
    /// `members` (only its members find it) or `workspace` (every
    /// workspace member may read it and join it).
    #[sqlx(try_from = "String")]
    pub visibility: StoredVisibility,
    pub archived: bool,
    /// Pass as expectedVersion to change it.
    pub version: i64,
    /// Your role in it; absent when you read it without being a member.
    pub role: Option<String>,
    /// The project or team space it is inside, when it is inside one you
    /// can read. Absent at the top.
    #[schemars(with = "Option<String>")]
    pub parent_id: Option<Uuid>,
    /// True when its people are its parent's: everyone in the parent is in
    /// it with the same role, and members are changed there. False when it
    /// has its own people.
    pub people_from_parent: bool,
    #[sqlx(skip)]
    pub url: String,
}

/// A stored kind, read through `SpaceKind`.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct StoredKind(pub SpaceKind);

impl TryFrom<String> for StoredKind {
    type Error = AppError;
    fn try_from(value: String) -> Result<Self, AppError> {
        SpaceKind::from_stored(&value)
            .map(Self)
            .ok_or(AppError::Internal("a space of an unknown kind"))
    }
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct StoredVisibility(pub SpaceVisibility);

impl TryFrom<String> for StoredVisibility {
    type Error = AppError;
    fn try_from(value: String) -> Result<Self, AppError> {
        SpaceVisibility::from_stored(&value)
            .map(Self)
            .ok_or(AppError::Internal("a space of an unknown visibility"))
    }
}

const VIEW_SELECT: &str = "SELECT space.id, space.workspace_id, space.kind, space.key, \
    space.name, space.description, space.icon, space.visibility, \
    space.archived_at IS NOT NULL AS archived, space.version, \
    (SELECT membership.role FROM project_memberships membership \
      WHERE membership.project_id = space.id AND membership.actor_id = $1 \
        AND membership.enabled) AS role, \
    CASE WHEN space_readable_by(space.parent_space_id, $1) THEN space.parent_space_id END \
      AS parent_id, \
    space.people_from_parent \
    FROM spaces space";

/// Every kind a space can be, as stored, for `kind = ANY(...)`.
fn space_kinds(kinds: &[SpaceKind]) -> Vec<&'static str> {
    kinds.iter().map(|kind| kind.stored()).collect()
}

fn with_url(state: &AppState, mut view: SpaceView) -> SpaceView {
    view.url = Addresses::of(state).url(
        &crate::events::public_origin(state),
        &WorkspaceAddress::id(view.workspace_id),
        &Address::space(view.id, Place::Home),
    );
    view
}

/// The space as `account` reads it: readable now, or archived and theirs.
async fn view(
    state: &AppState,
    connection: &mut PgConnection,
    account: Uuid,
    space_id: Uuid,
) -> Result<Option<SpaceView>, AppError> {
    let row = sqlx::query_as::<_, SpaceView>(&format!(
        "{VIEW_SELECT} WHERE space.id = $2 AND space.kind = ANY($3) \
           AND (space_readable_by(space.id, $1) \
                OR (space.archived_at IS NOT NULL AND EXISTS ( \
                    SELECT 1 FROM project_memberships membership \
                    WHERE membership.project_id = space.id AND membership.actor_id = $1 \
                      AND membership.enabled)))"
    ))
    .bind(account)
    .bind(space_id)
    .bind(space_kinds(&SpaceKind::ALL))
    .fetch_optional(&mut *connection)
    .await?;
    Ok(row.map(|row| with_url(state, row)))
}

/// The space for a call in `workspace`: readable by the caller, in that
/// workspace, and within a credential held to one space.
async fn readable_here(call: &mut OpCall<'_>, space_id: Uuid) -> Result<SpaceView, OpError> {
    let caller = call.ctx.caller.clone();
    let workspace_id = call.ctx.workspace_id;
    let state = call.state;
    if caller
        .confined_space
        .is_some_and(|confined| confined != space_id)
    {
        return Err(OpError::not_found("That space"));
    }
    let tx = call.tx().await?;
    view(state, tx, caller.account_id, space_id)
        .await?
        .filter(|space| space.workspace_id == workspace_id)
        .map(|space| within_reach(&caller, space))
        .ok_or_else(|| OpError::not_found("That space"))
}

/// A credential confined to one space isn't told what that space is inside.
fn within_reach(caller: &access::Caller, mut space: SpaceView) -> SpaceView {
    if caller.confined_space.is_some()
        || space
            .parent_id
            .is_some_and(|parent| !limit_allows(caller, parent))
    {
        space.parent_id = None;
    }
    space
}

/// The spaces a credential's own limit leaves it; `None` when it has none.
/// A token narrowed to some spaces reads nothing about the others, though
/// its account is in them.
fn limited_to(caller: &access::Caller) -> Option<Vec<Uuid>> {
    caller
        .limits
        .as_ref()
        .map(|limits| limits.projects.keys().copied().collect())
}

fn limit_allows(caller: &access::Caller, space: Uuid) -> bool {
    caller
        .limits
        .as_ref()
        .is_none_or(|limits| limits.projects.contains_key(&space))
}

/// One workspace's tree changes take turns. Taken before any space's row,
/// so a move and an archive that touch the same spaces can't wait on each
/// other.
async fn lock_tree(call: &mut OpCall<'_>) -> Result<(), OpError> {
    let workspace_id = call.ctx.workspace_id;
    let tx = call.tx().await?;
    manage::lock_tree(tx, workspace_id).await?;
    Ok(())
}

fn refused(error: AppError) -> OpError {
    match error {
        AppError::ProjectKeyConflict => OpError::Conflict {
            reason: crate::ops::ConflictReason::State {
                code: "space_key_taken",
            },
            message: "That key is already used by a space in this workspace. Pick another key."
                .into(),
            current: None,
        },
        AppError::ProjectHasActiveRuns => OpError::Conflict {
            reason: crate::ops::ConflictReason::State {
                code: "space_has_active_runs",
            },
            message: "A run is still going in that space. Wait for its runs to finish, or \
                interrupt them, then archive it."
                .into(),
            current: None,
        },
        AppError::Validation("Project key") => OpError::invalid(
            "key",
            "Use 2 to 10 letters or digits, starting with a letter, such as \"WEB\".",
        ),
        AppError::Validation("Project name") => OpError::invalid(
            "name",
            "Give the space a name of 1 to 120 bytes, on one line.",
        ),
        AppError::Validation("description") => OpError::invalid(
            "description",
            "Keep the description to 500 characters on one line.",
        ),
        AppError::Validation("icon") => OpError::invalid(
            "icon",
            "Give one emoji, or an empty string to take the icon away.",
        ),
        error => error.into(),
    }
}

fn check_version(expected: Option<i64>, current: &Locked) -> Result<(), OpError> {
    match expected {
        Some(expected) if expected != current.version => Err(AppError::VersionConflict {
            resource: "space",
            current_version: current.version,
        }
        .into()),
        _ => Ok(()),
    }
}

/// The refusal for a check made again under the space's lock: what the
/// caller holds there now, after any change they waited behind.
fn denied_here(denied: access::Denied, verb: &str) -> OpError {
    match denied {
        access::Denied::NotFound => OpError::not_found("That space"),
        access::Denied::Permission {
            because: access::DenyReason::ReadOnlyCredential,
            ..
        } => OpError::Denied {
            reason: DenyReason::ReadOnlyCredential,
            message: "This connection is read-only. Reconnect it with full access to make \
                changes."
                .into(),
        },
        access::Denied::Permission {
            because: access::DenyReason::Delegation,
            ..
        } => OpError::Denied {
            reason: DenyReason::Delegation,
            message: format!("This app can't {verb} spaces for you.").into(),
        },
        _ => OpError::Denied {
            reason: DenyReason::Role {
                permission: Perm::ProjectsManage,
            },
            message: format!(
                "To {verb} a space you need \"Manage projects\" in it, which your role there \
                 doesn't give you."
            )
            .into(),
        },
    }
}

/// "Manage projects" in the space, checked again with the membership rows
/// locked, so a role taken away while this call waited for the space's lock
/// counts.
async fn recheck_manage(call: &mut OpCall<'_>, space_id: Uuid, verb: &str) -> Result<(), OpError> {
    let caller = call.ctx.caller.clone();
    let tx = call.tx().await?;
    let need = Need::permission(Perm::ProjectsManage).writes();
    access::check_locked(tx, &caller, need, access::Target::Space(space_id))
        .await?
        .map(|_| ())
        .map_err(|denied| denied_here(denied, verb))
}

/// Whose people a space inside another has.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum People {
    /// Everyone in its parent, with the role they hold there, kept in step.
    Parent,
    /// Only the people added to it.
    Own,
}

/// "Manage projects" in the space something is being put inside. A parent
/// the caller can't read is not found.
async fn check_parent_manage(call: &mut OpCall<'_>, parent: Uuid) -> Result<(), OpError> {
    let caller = call.ctx.caller.clone();
    let workspace_id = call.ctx.workspace_id;
    if caller
        .confined_space
        .is_some_and(|confined| confined != parent)
    {
        return Err(OpError::not_found("That parent project"));
    }
    let tx = call.tx().await?;
    let in_workspace = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM spaces WHERE id = $1 AND workspace_id = $2)",
    )
    .bind(parent)
    .bind(workspace_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::from)?;
    if !in_workspace {
        return Err(OpError::not_found("That parent project"));
    }
    let need = Need::permission(Perm::ProjectsManage).writes();
    match access::check_locked(tx, &caller, need, access::Target::Space(parent)).await? {
        Ok(_) => Ok(()),
        Err(access::Denied::NotFound) => Err(OpError::not_found("That parent project")),
        Err(
            denied @ access::Denied::Permission {
                because: access::DenyReason::ReadOnlyCredential | access::DenyReason::Delegation,
                ..
            },
        ) => Err(denied_here(denied, "move")),
        Err(_) => Err(OpError::Denied {
            reason: DenyReason::Role {
                permission: Perm::ProjectsManage,
            },
            message: "To put a project inside another you need \"Manage projects\" in that \
                one, which your role there doesn't give you."
                .into(),
        }),
    }
}

/// The space locked for a change in this call's workspace.
async fn locked(call: &mut OpCall<'_>, space_id: Uuid) -> Result<Locked, OpError> {
    let workspace_id = call.ctx.workspace_id;
    let tx = call.tx().await?;
    manage::lock(tx, space_id)
        .await?
        .filter(|space| space.workspace_id == workspace_id)
        .ok_or_else(|| OpError::not_found("That space"))
}

async fn answer(call: &mut OpCall<'_>, space_id: Uuid) -> Result<SpaceView, OpError> {
    let caller = call.ctx.caller.clone();
    let state = call.state;
    let tx = call.tx().await?;
    view(state, tx, caller.account_id, space_id)
        .await?
        .map(|space| within_reach(&caller, space))
        .ok_or_else(|| OpError::not_found("That space"))
}

// ---------------------------------------------------------------------------
// space_list

pub struct SpaceList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceListInput {
    /// Only these kinds: `workspace`, `project`, `team`, `personal`.
    /// Default: every kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<SpaceKind>>,
    /// Also list archived spaces you're a member of, which space_restore
    /// brings back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_archived: bool,
}

impl Targeted for SpaceListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceListView {
    pub spaces: Vec<SpaceView>,
}

impl Operation for SpaceList {
    const NAME: &'static str = "space_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List spaces";
    const DESCRIPTION: &'static str = "The spaces in this workspace you can read, by name: the \
        workspace's own space, projects, team spaces and your personal space, with each one's \
        key, icon, who may find it, your role, and the project it is inside (parentId) when it \
        is a sub-project. includeArchived also lists archived ones you're a member of.";

    type Input = SpaceListInput;
    type Output = SpaceListView;

    async fn run(call: &mut OpCall<'_>, input: SpaceListInput) -> Result<SpaceListView, OpError> {
        let caller = call.ctx.caller.clone();
        let workspace_id = call.ctx.workspace_id;
        let state = call.state;
        let kinds = space_kinds(input.kinds.as_deref().unwrap_or(&SpaceKind::ALL));
        let tx = call.tx().await?;
        let rows = sqlx::query_as::<_, SpaceView>(&format!(
            "{VIEW_SELECT} WHERE space.workspace_id = $2 AND space.kind = ANY($3) \
               AND (space.id IN (SELECT spaces_readable_by($1, $2)) \
                    OR ($4 AND space.archived_at IS NOT NULL AND EXISTS ( \
                        SELECT 1 FROM project_memberships membership \
                        WHERE membership.project_id = space.id \
                          AND membership.actor_id = $1 AND membership.enabled))) \
               AND ($5::uuid IS NULL OR space.id = $5) \
             ORDER BY space.archived_at IS NOT NULL, lower(space.name), space.id \
             LIMIT 500"
        ))
        .bind(caller.account_id)
        .bind(workspace_id)
        .bind(&kinds)
        .bind(input.include_archived)
        .bind(caller.confined_space)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        Ok(SpaceListView {
            spaces: rows
                .into_iter()
                .map(|row| with_url(state, within_reach(&caller, row)))
                .collect(),
        })
    }
}

// ---------------------------------------------------------------------------
// space_open

pub struct SpaceOpen;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceOpenInput {
    /// The space: its id, link or key (such as "WEB").
    pub space_id: IdRef,
}

impl Targeted for SpaceOpenInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for SpaceOpen {
    const NAME: &'static str = "space_open";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Open a space";
    const DESCRIPTION: &'static str = "One space you can read: its kind, key, name, description, \
        icon, who may find it, your role and its version.";

    type Input = SpaceOpenInput;
    type Output = SpaceView;

    async fn run(call: &mut OpCall<'_>, input: SpaceOpenInput) -> Result<SpaceView, OpError> {
        readable_here(call, input.space_id.id()).await
    }
}

// ---------------------------------------------------------------------------
// space_tree_get (AWD-221)

pub struct SpaceTreeGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceTreeGetInput {
    /// The space: its id, link or key (such as "WEB").
    pub space_id: IdRef,
}

impl Targeted for SpaceTreeGetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// How a space's tasks stand, as the reader sees them.
#[derive(Clone, Copy, Debug, Default, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskCounts {
    /// Not finished and not canceled.
    pub open: i64,
    /// Finished.
    pub done: i64,
}

/// A space above the one asked for.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceAbove {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub icon: Option<String>,
    pub url: String,
}

/// A space inside the one asked for, at any depth.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceInside {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// The space it is directly inside; absent when you can't read that one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub parent_id: Option<Uuid>,
    /// 1 when it is directly inside the space asked for, 2 one further down.
    pub depth: i32,
    pub key: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    /// True when its people are its parent's.
    pub people_from_parent: bool,
    /// Your role in it; absent when you read it without being a member.
    pub role: Option<String>,
    /// Its own tasks, where you may see tasks there and Tasks is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<TaskCounts>,
    pub url: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceTreeView {
    pub space: SpaceView,
    /// The spaces it is inside that you can read, outermost first. Empty at
    /// the top.
    pub path: Vec<SpaceAbove>,
    /// The spaces inside it that you can read, parents before what is in
    /// them, by name. task_list with includeSubProjects lists their tasks.
    pub inside: Vec<SpaceInside>,
    /// The space's own tasks, where you may see them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<TaskCounts>,
    /// Its tasks and those of everything inside it that you may see.
    pub tasks_with_inside: TaskCounts,
}

#[derive(sqlx::FromRow)]
struct InsideRow {
    id: Uuid,
    parent_id: Uuid,
    key: String,
    name: String,
    description: String,
    icon: Option<String>,
    people_from_parent: bool,
    role: Option<String>,
}

/// Open and finished tasks in each of `spaces` where `caller` may see tasks
/// and Tasks is on for them.
async fn task_counts(
    connection: &mut PgConnection,
    caller: &access::Caller,
    spaces: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, TaskCounts>, AppError> {
    let modules =
        crate::modules::state::ProjectModules::load(&mut *connection, caller.account_id, spaces)
            .await?;
    // Counted where the task list itself works.
    let showing = modules.keep(spaces, |set, direct| {
        !direct && set.allows_tool("task_list")
    });
    let filter = access::sql_filter(caller, Perm::TasksView, "space.id", 2);
    let sql = format!(
        r#"
        SELECT space.id,
               count(task.id) FILTER (
                   WHERE status.semantic_category NOT IN ('completed', 'canceled')) AS open,
               count(task.id) FILTER (WHERE status.semantic_category = 'completed') AS done
        FROM spaces space
        LEFT JOIN task_rows task ON task.project_id = space.id AND task.archived_at IS NULL
        LEFT JOIN work_item_statuses status
          ON status.project_id = task.project_id AND status.id = task.status_id
        WHERE space.id = ANY($1) AND {}
        GROUP BY space.id
        "#,
        filter.sql
    );
    let mut query = sqlx::query_as::<_, (Uuid, i64, i64)>(&sql)
        .bind(&showing)
        .bind(filter.account_id)
        .bind(filter.connection_id)
        .bind(filter.permission);
    if let Some(limited_to) = &filter.limited_to {
        query = query.bind(limited_to);
    }
    Ok(query
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(|(id, open, done)| (id, TaskCounts { open, done }))
        .collect())
}

impl Operation for SpaceTreeGet {
    const NAME: &'static str = "space_tree_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Open a space's tree";
    const DESCRIPTION: &'static str = "One space with the projects it is inside and the \
        projects inside it (sub-projects, at any depth) that you can read, each with its open \
        and finished task counts where you may see its tasks, and the totals for the whole \
        tree. Make a sub-project with space_create parentId; move one with space_update.";

    type Input = SpaceTreeGetInput;
    type Output = SpaceTreeView;

    async fn run(
        call: &mut OpCall<'_>,
        input: SpaceTreeGetInput,
    ) -> Result<SpaceTreeView, OpError> {
        let space = readable_here(call, input.space_id.id()).await?;
        let caller = call.ctx.caller.clone();
        if !limit_allows(&caller, space.id) {
            return Err(OpError::not_found("That space"));
        }
        let limited = limited_to(&caller);
        let state = call.state;
        let tx = call.tx().await?;
        let url = |id: Uuid| {
            Addresses::of(state).url(
                &crate::events::public_origin(state),
                &WorkspaceAddress::id(space.workspace_id),
                &Address::space(id, Place::Home),
            )
        };
        // What it is inside, as far up as the caller can read.
        let above = sqlx::query_as::<_, (Uuid, String, String, Option<String>)>(
            r#"
            WITH RECURSIVE up AS (
                SELECT parent.id, parent.key, parent.name, parent.icon,
                       parent.parent_space_id, 1 AS level
                FROM spaces child JOIN spaces parent ON parent.id = child.parent_space_id
                WHERE child.id = $1 AND space_readable_by(parent.id, $2)
                  AND $3::uuid IS NULL
                  AND ($4::uuid[] IS NULL OR parent.id = ANY($4))
                UNION ALL
                SELECT parent.id, parent.key, parent.name, parent.icon,
                       parent.parent_space_id, up.level + 1
                FROM up JOIN spaces parent ON parent.id = up.parent_space_id
                WHERE space_readable_by(parent.id, $2) AND up.level < 16
                  AND ($4::uuid[] IS NULL OR parent.id = ANY($4))
            )
            SELECT id, key, name, icon FROM up ORDER BY level DESC
            "#,
        )
        .bind(space.id)
        .bind(caller.account_id)
        // A credential confined to one space reads nothing above it.
        .bind(caller.confined_space)
        .bind(&limited)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let below = super::access::below(tx, &[space.id]).await?;
        let below_ids: Vec<Uuid> = below.iter().map(|(id, _, _)| *id).collect();
        let depth_of: std::collections::HashMap<Uuid, i32> =
            below.iter().map(|(id, _, depth)| (*id, *depth)).collect();
        let rows = sqlx::query_as::<_, InsideRow>(
            "SELECT space.id, space.parent_space_id AS parent_id, space.key, \
                    space.name, space.description, space.icon, space.people_from_parent, \
                    (SELECT membership.role FROM project_memberships membership \
                      WHERE membership.project_id = space.id AND membership.actor_id = $2 \
                        AND membership.enabled) AS role \
             FROM spaces space \
             WHERE space.id = ANY($1) AND space_readable_by(space.id, $2) \
               AND ($3::uuid IS NULL OR space.id = $3) \
               AND ($4::uuid[] IS NULL OR space.id = ANY($4)) \
             ORDER BY lower(space.name), space.id",
        )
        .bind(&below_ids)
        .bind(caller.account_id)
        .bind(caller.confined_space)
        .bind(&limited)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let mut counted: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
        counted.push(space.id);
        let counts = task_counts(tx, &caller, &counted).await?;
        let mut total = TaskCounts::default();
        for count in counts.values() {
            total.open += count.open;
            total.done += count.done;
        }
        let shown: std::collections::HashSet<Uuid> = counted.iter().copied().collect();
        // Parents before what is in them: by depth, then by name as read.
        let mut inside: Vec<SpaceInside> = rows
            .into_iter()
            .map(|row| SpaceInside {
                depth: depth_of.get(&row.id).copied().unwrap_or(1),
                tasks: counts.get(&row.id).copied(),
                url: url(row.id),
                id: row.id,
                parent_id: Some(row.parent_id).filter(|parent| shown.contains(parent)),
                key: row.key,
                name: row.name,
                description: row.description,
                icon: row.icon,
                people_from_parent: row.people_from_parent,
                role: row.role,
            })
            .collect();
        inside.sort_by_key(|row| row.depth);
        Ok(SpaceTreeView {
            path: above
                .into_iter()
                .map(|(id, key, name, icon)| SpaceAbove {
                    url: url(id),
                    id,
                    key,
                    name,
                    icon,
                })
                .collect(),
            tasks: counts.get(&space.id).copied(),
            tasks_with_inside: total,
            inside,
            space,
        })
    }
}

// ---------------------------------------------------------------------------
// space_personal_open (K4.9)

pub struct SpacePersonalOpen;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpacePersonalOpenInput {}

impl Targeted for SpacePersonalOpenInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for SpacePersonalOpen {
    const NAME: &'static str = "space_personal_open";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Open my space";
    const DESCRIPTION: &'static str = "Your personal space in the workspace, made the first time \
        you open it: your drafts and notes, read by nobody else. Other tools reach it with \
        spaceId or projectId \"me\".";

    type Input = SpacePersonalOpenInput;
    type Output = SpaceView;

    async fn run(
        call: &mut OpCall<'_>,
        _input: SpacePersonalOpenInput,
    ) -> Result<SpaceView, OpError> {
        let caller = call.ctx.caller.clone();
        let workspace_id = call.ctx.workspace_id;
        let tx = call.tx().await?;
        let space = super::personal::personal(tx, workspace_id, caller.account_id)
            .await
            .map_err(refused)?
            .ok_or_else(|| OpError::not_found("Your space in that workspace"))?;
        if caller
            .confined_space
            .is_some_and(|confined| confined != space)
        {
            return Err(OpError::not_found("Your space in that workspace"));
        }
        readable_here(call, space).await
    }
}

// ---------------------------------------------------------------------------
// space_create

pub struct SpaceCreate;

/// The kinds `space_create` makes.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CreatableKind {
    Project,
    Team,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceCreateInput {
    /// `project` (the default) or `team`. The workspace's own space comes
    /// with the workspace, and personal spaces with their account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<CreatableKind>,
    /// A short key for task numbers, such as "WEB": 2 to 10 letters or
    /// digits, starting with a letter. It can't be changed later.
    pub key: String,
    pub name: String,
    /// Up to 500 characters on one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// An emoji shown beside the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// `members` (the default: only its members find it) or `workspace`
    /// (every workspace member may read it and join it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<SpaceVisibility>,
    /// A project template of the workspace to start from, by id
    /// (project_template_list): its workflow, labels, channels, settings
    /// and task templates are set up in the new space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<IdRef>,
    /// What it starts with: channels and pages. Leave it out for its kind's
    /// own (a team space starts with #general and a guide page).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<super::shape::SpaceShape>,
    /// Make it inside this project or team space (its id or link), as
    /// a sub-project with its own tasks, chat and pages. You need "Manage
    /// projects" there. Three levels deep at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<IdRef>,
    /// With parentId, whose people it has: `parent` (the default: everyone
    /// in the parent, with their role there, kept in step) or `own` (only
    /// you to start, and whoever you add).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub people: Option<People>,
}

impl Targeted for SpaceCreateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for SpaceCreate {
    const NAME: &'static str = "space_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ProjectsCreate);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.created"];
    const TITLE: &'static str = "Make a space";
    const DESCRIPTION: &'static str = "Make a project or a team space in this workspace; you \
        become its owner. Give it a key for task numbers (it can't change later), a name, and \
        optionally a description, an icon, who may find it and a project template to start \
        from. With parentId it is made inside another project as a sub-project, with that \
        project's people unless people is own. Returns the space.";

    type Input = SpaceCreateInput;
    type Output = SpaceView;

    async fn run(call: &mut OpCall<'_>, input: SpaceCreateInput) -> Result<SpaceView, OpError> {
        let key = manage::normalize_key(&input.key).map_err(refused)?;
        let name = manage::normalize_name(&input.name).map_err(refused)?;
        let description = manage::normalize_description(input.description.as_deref().unwrap_or(""))
            .map_err(refused)?;
        let icon = manage::normalize_icon(input.icon.as_deref().unwrap_or("")).map_err(refused)?;
        let kind = match input.kind.unwrap_or(CreatableKind::Project) {
            CreatableKind::Project => SpaceKind::Project,
            CreatableKind::Team => SpaceKind::Team,
        };
        let workspace_id = call.ctx.workspace_id;
        let account = call.account_id();
        // A project needs Projects on in the workspace; a team space doesn't.
        if kind == SpaceKind::Project {
            let caller = call.ctx.caller.clone();
            let tx = call.tx().await?;
            let need = Need::maybe(None).writing(true).in_module("projects");
            if let Err(access::Denied::ModuleOff { module }) =
                access::check(tx, &caller, need, access::Target::Workspace(workspace_id)).await?
            {
                return Err(OpError::Denied {
                    reason: DenyReason::ModuleOff {
                        module: module.to_owned(),
                    },
                    message: "Projects is turned off in this workspace. Make a team space, or \
                        turn Projects on."
                        .into(),
                });
            }
        }
        let inside = match input.parent_id.map(IdRef::id) {
            Some(parent) => {
                check_parent_manage(call, parent).await?;
                Some(manage::Inside {
                    parent,
                    people_from_parent: input.people != Some(People::Own),
                })
            }
            None if input.people == Some(People::Parent) => {
                return Err(OpError::invalid(
                    "people",
                    "Only a project inside another can have that one's people: pass parentId \
                     too.",
                ));
            }
            None => None,
        };
        let mut actor = crate::work_item_commands::CommandActor::from(call.ctx);
        let tx = call.tx().await?;
        // The owner is named as they are in this workspace, whatever their
        // kind.
        let (display_name, login_name) = crate::accounts::names_in(tx, workspace_id, account)
            .await?
            .ok_or_else(|| OpError::not_found("That workspace"))?;
        let space_id = manage::create(
            tx,
            NewSpace {
                workspace_id,
                kind,
                key: &key,
                name: &name,
                description: &description,
                visibility: input.visibility.unwrap_or(SpaceVisibility::Members),
                icon: icon.as_deref(),
                owner: Owner {
                    account_id: account,
                    display_name: &display_name,
                    login_name: &login_name,
                },
                inside,
            },
        )
        .await
        .map_err(refused)?;
        let template = match input.template_id.map(IdRef::id) {
            Some(template_id) => {
                actor.project_id = space_id;
                match crate::work_items::project_templates::apply_to_new_project(
                    tx,
                    actor,
                    workspace_id,
                    template_id,
                )
                .await
                {
                    Ok(_) => Some(template_id),
                    Err(AppError::NotFound(_)) => {
                        return Err(OpError::not_found(
                            "That project template (see project_template_list)",
                        ));
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            None => None,
        };
        // K4.20: its starting shape, its kind's own unless one is given.
        let shape = input.shape.unwrap_or_else(|| super::shape::builtin(kind));
        let writer = crate::memory::Writer::from(call.ctx);
        let tx = call.tx().await?;
        super::shape::apply(tx, writer, space_id, &shape)
            .await
            .map_err(refused)?;
        call.audit_target(
            "space",
            space_id,
            json!({
                "kind": kind,
                "key": key,
                "template": template,
                "parent": inside.map(|inside| inside.parent),
            }),
        );
        answer(call, space_id).await
    }
}

// ---------------------------------------------------------------------------
// space_update

pub struct SpaceUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceUpdateInput {
    /// The space: its id, link or key.
    pub space_id: IdRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// An emoji, or an empty string to take the icon away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// `members` or `workspace`; projects and team spaces only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<SpaceVisibility>,
    /// Move it inside this project or team space (its id or link).
    /// What is under it moves with it. You need "Manage projects" there
    /// too. Three levels deep at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<IdRef>,
    /// true moves it back to the top, out of the project it is inside. It
    /// keeps the people it has, as its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_parent: bool,
    /// For a project inside another: `parent` gives it its parent's people
    /// (everyone there, with their role, kept in step; people it had who
    /// aren't in the parent leave unless they were added by hand), `own`
    /// lets it keep the people it has and change them by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub people: Option<People>,
    /// The version you last read. Leave it out to change whatever the
    /// current version is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for SpaceUpdateInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for SpaceUpdate {
    const NAME: &'static str = "space_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ProjectsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.updated"];
    const TITLE: &'static str = "Change a space";
    const DESCRIPTION: &'static str = "Rename a space, or change its description, icon (an \
        emoji) or who may find it. parentId moves it inside another project, clearParent moves \
        it back to the top, and people says whether it has its parent's people or its own. The \
        key stays. Pass expectedVersion from what you read. Returns the space.";

    type Input = SpaceUpdateInput;
    type Output = SpaceView;

    async fn run(call: &mut OpCall<'_>, input: SpaceUpdateInput) -> Result<SpaceView, OpError> {
        let changes = Changes {
            name: input
                .name
                .as_deref()
                .map(manage::normalize_name)
                .transpose()
                .map_err(refused)?,
            description: input
                .description
                .as_deref()
                .map(manage::normalize_description)
                .transpose()
                .map_err(refused)?,
            icon: input
                .icon
                .as_deref()
                .map(manage::normalize_icon)
                .transpose()
                .map_err(refused)?,
            visibility: input.visibility,
            parent: match (input.parent_id.as_ref(), input.clear_parent) {
                (Some(_), true) => {
                    return Err(OpError::invalid(
                        "clearParent",
                        "Pass parentId to move it inside a project, or clearParent to move it \
                         to the top, not both.",
                    ));
                }
                (Some(parent), false) => Some(Some(parent.id())),
                (None, true) => Some(None),
                (None, false) => None,
            },
            people_from_parent: input.people.map(|people| people == People::Parent),
        };
        let space_id = input.space_id.id();
        let account = call.account_id();
        if changes.parent.is_some() || changes.people_from_parent.is_some() {
            lock_tree(call).await?;
        }
        let current = locked(call, space_id).await?;
        recheck_manage(call, space_id, "change").await?;
        check_version(input.expected_version, &current)?;
        // Going inside another space is a change to that one too, and so is
        // taking its people: both need "Manage projects" there.
        let parent_after = changes.parent.unwrap_or(current.parent_space_id);
        let moves = changes.parent.is_some() && parent_after != current.parent_space_id;
        let takes_people = changes.people_from_parent == Some(true) && !current.people_from_parent;
        if let Some(parent) = parent_after
            && (moves || takes_people)
        {
            check_parent_manage(call, parent).await?;
        }
        let tx = call.tx().await?;
        let (changed, _) = manage::update(tx, account, space_id, &current, changes)
            .await
            .map_err(refused)?;
        call.audit_target(
            "space",
            space_id,
            json!({ "changes": changed, "previousVersion": current.version }),
        );
        answer(call, space_id).await
    }
}

// ---------------------------------------------------------------------------
// space_archive and space_restore

pub struct SpaceArchive;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceArchiveInput {
    /// The space: its id, link or key.
    pub space_id: IdRef,
    /// The version you last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for SpaceArchiveInput {
    fn target(&self) -> Target {
        Target::Space(self.space_id.id())
    }
}

impl Operation for SpaceArchive {
    const NAME: &'static str = "space_archive";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ProjectsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.archived"];
    const TITLE: &'static str = "Archive a space";
    const DESCRIPTION: &'static str = "Archive a project or team space, with the projects \
        inside it: they leave everyone's lists and nothing in them changes until space_restore \
        brings them back. A space with a run still going, in it or under it, isn't archived. \
        Returns the space.";

    type Input = SpaceArchiveInput;
    type Output = SpaceView;

    async fn run(call: &mut OpCall<'_>, input: SpaceArchiveInput) -> Result<SpaceView, OpError> {
        let space_id = input.space_id.id();
        let account = call.account_id();
        lock_tree(call).await?;
        let current = locked(call, space_id).await?;
        recheck_manage(call, space_id, "archive").await?;
        if !current.archived {
            check_version(input.expected_version, &current)?;
        }
        if !current.archived {
            // What is inside goes with it, so that takes "Manage projects"
            // in each of those too: a project with its own people isn't
            // archived by someone who only manages the one around it.
            let below = {
                let tx = call.tx().await?;
                super::access::below(tx, &[space_id]).await?
            };
            for (inside, _, _) in below {
                if recheck_manage(call, inside, "archive").await.is_err() {
                    return Err(refused(AppError::Conflict(
                        "space",
                        "there are projects inside this one that you don't manage; \
                         move them out or ask whoever manages them, then archive it",
                    )));
                }
            }
        }
        let tx = call.tx().await?;
        manage::archive(tx, account, space_id, &current)
            .await
            .map_err(refused)?;
        call.audit_target(
            "space",
            space_id,
            json!({ "previousVersion": current.version }),
        );
        answer(call, space_id).await
    }
}

pub struct SpaceRestore;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceRestoreInput {
    /// The archived space: its id or link (an archived space's key no
    /// longer finds it).
    pub space_id: IdRef,
    /// The version you last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for SpaceRestoreInput {
    // An archived space is closed to every other call: the target is its
    // workspace, found through the caller's membership, and the space is
    // checked as it was archived.
    fn target(&self) -> Target {
        Target::thing("member_space", self.space_id.id())
    }
}

impl Operation for SpaceRestore {
    const NAME: &'static str = "space_restore";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["space.restored"];
    const TITLE: &'static str = "Restore a space";
    const DESCRIPTION: &'static str = "Bring an archived space back as it was, with the \
        projects inside it that were archived with it and that you manage, for someone who \
        could manage it. Find \
        archived spaces with space_list includeArchived. Returns the space.";

    type Input = SpaceRestoreInput;
    type Output = SpaceView;

    async fn run(call: &mut OpCall<'_>, input: SpaceRestoreInput) -> Result<SpaceView, OpError> {
        let space_id = input.space_id.id();
        let caller = call.ctx.caller.clone();
        if caller
            .confined_space
            .is_some_and(|confined| confined != space_id)
        {
            return Err(OpError::not_found("That space"));
        }
        lock_tree(call).await?;
        let current = locked(call, space_id).await?;
        let tx = call.tx().await?;
        let need = Need::permission(Perm::ProjectsManage).writes();
        let checked = if current.archived {
            access::check_archived_space(tx, &caller, need, space_id).await?
        } else {
            access::check_locked(tx, &caller, need, access::Target::Space(space_id)).await?
        };
        checked
            .map(|_| ())
            .map_err(|denied| denied_here(denied, "restore"))?;
        let mut with_it = Vec::new();
        if current.archived {
            check_version(input.expected_version, &current)?;
            // What was archived with it comes back with it, where the
            // caller manages that too. A project inside that isn't theirs
            // stays archived, with what is inside it, for whoever manages
            // it to bring back; nothing here says it exists.
            let inside = manage::archived_with(tx, space_id, current.archived_at)
                .await
                .map_err(refused)?;
            let mut back = vec![space_id];
            for (id, parent) in inside {
                if caller.confined_space.is_some() || !back.contains(&parent) {
                    continue;
                }
                let need = Need::permission(Perm::ProjectsManage).writes();
                if access::check_archived_space(tx, &caller, need, id)
                    .await?
                    .is_ok()
                {
                    back.push(id);
                }
            }
            with_it = back.split_off(1);
        }
        manage::restore(tx, caller.account_id, space_id, &current, &with_it)
            .await
            .map_err(refused)?;
        call.audit_target(
            "space",
            space_id,
            json!({ "previousVersion": current.version }),
        );
        answer(call, space_id).await
    }
}

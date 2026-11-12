//! Whiteboard tools: find, read, draw on and look at the workspace's
//! Excalidraw boards with no browser open. Edits are operations (add,
//! connect, align, layout…) that the server turns into Excalidraw elements
//! and merges like a browser's change, so open boards update live.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rmcp::{
    model::{CallToolResult, ContentBlock},
    schemars,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, confined_project, parse_id, public_url},
};
use crate::{
    AppState,
    error::AppError,
    whiteboards::{
        self, BoardChange, BoardSummary, Caller, LinkView, ListFilter, OnIn, Placement, TargetKind,
        edit::{self, EditError, EditOutcome},
        render,
        scene::{self, ElementSummary, Operation, Scene},
        store::{self, Editor, RevisionSummary},
    },
};

/// Most element summaries whiteboard_read returns.
const MAX_READ_ELEMENTS: usize = 2_000;

pub(super) fn caller(principal: &McpPrincipal) -> Caller {
    Caller {
        actor_id: principal.actor_id,
        actor_kind: principal.actor_kind,
        via_grant_id: principal.via_grant_id(),
        connection_id: principal.external_agent_connection_id,
        confined_project: confined_project(principal),
        channel: "mcp",
        authentication_class: principal.authentication_class,
        session_id: None,
    }
}

fn board_url(state: &AppState, id: Uuid) -> String {
    public_url(state, &format!("/whiteboards/{id}"))
}

fn whiteboard_id(value: &str) -> Option<Uuid> {
    parse_id(value.trim())
}

fn refused<T>(error: AppError, operation: &'static str) -> ToolResult<T> {
    match error {
        AppError::NotFound("whiteboard") => ToolResult::not_found("That whiteboard"),
        AppError::NotFound("link target") => ToolResult::not_found("That task, project or page"),
        AppError::Forbidden => {
            ToolResult::denied("You can look at this whiteboard but not change it.")
        }
        error => ToolResult::from_error(error, operation),
    }
}

/// AWD-75: where Whiteboards is on for the caller: the projects they reach,
/// their role in each narrowing it, and the workspaces they're in, for
/// boards across one.
async fn whiteboards_on(state: &AppState, principal: &McpPrincipal) -> Result<OnIn, AppError> {
    let projects = super::common::accessible_project_ids(state, principal).await?;
    let mut connection = state.pool.acquire().await?;
    let in_projects =
        crate::modules::state::ProjectModules::load(&mut connection, principal.actor_id, &projects)
            .await?;
    let workspaces = crate::modules::state::by_workspace(&mut connection, principal.actor_id)
        .await?
        .into_iter()
        .filter(|(_, set)| set.contains("whiteboards"))
        .map(|(workspace, _)| workspace)
        .collect();
    Ok(OnIn {
        projects: in_projects.with(&projects, "whiteboards"),
        workspaces,
    })
}

/// AWD-75: whether Whiteboards is off for the caller where whiteboard `id`
/// lives: its project, their role there narrowing it, or for a board across
/// the workspace, the workspace and their workspace role. Asked only once
/// `whiteboards::access` lets them at the board, so it says nothing about
/// boards they can't reach: when access is refused it answers false and
/// leaves that answer to the tool, which checks access itself. A failed
/// module lookup is an error, never "on".
pub(super) async fn board_module_off(
    state: &AppState,
    caller: &Caller,
    id: Uuid,
) -> Result<bool, AppError> {
    let mut connection = state.pool.acquire().await?;
    // A board the caller can't reach is left to the tool's own refusal, so
    // the answer says nothing more about it; any other failure (the database)
    // stops here instead of skipping the module check.
    let board = match whiteboards::access(&mut connection, caller, id, false).await {
        Ok(board) => board,
        Err(AppError::NotFound(_) | AppError::Forbidden) => return Ok(false),
        Err(error) => return Err(error),
    };
    let modules = match board.project_id {
        Some(project) => {
            crate::modules::state::for_project(&mut connection, project, caller.actor_id)
                .await?
                .unwrap_or_default()
        }
        None => crate::modules::state::by_workspace(&mut connection, caller.actor_id)
            .await?
            .get(&board.organization_id)
            .copied()
            .unwrap_or_default(),
    };
    Ok(!modules.contains("whiteboards"))
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WhiteboardLine {
    #[serde(flatten)]
    pub whiteboard: BoardSummary,
    pub url: String,
}

fn line(state: &AppState, whiteboard: BoardSummary) -> WhiteboardLine {
    WhiteboardLine {
        url: board_url(state, whiteboard.id),
        whiteboard,
    }
}

// ---------------------------------------------------------------------------
// whiteboard_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WhiteboardListArguments {
    /// Words in a board's title or on the board.
    pub query: Option<String>,
    /// Only boards in this project.
    pub project_id: Option<String>,
    /// Only boards in this workspace (organization ID).
    pub workspace_id: Option<String>,
    /// Only boards linked to this: task, project, or page.
    pub linked_to_kind: Option<String>,
    /// The task, project or page's ID (with linkedToKind).
    pub linked_to_id: Option<String>,
    /// Boards in the trash instead.
    pub deleted: Option<bool>,
    /// How many (default 50, at most 200).
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WhiteboardListValue {
    pub whiteboards: Vec<WhiteboardLine>,
}

pub(super) async fn whiteboard_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WhiteboardListArguments,
) -> ToolResult<WhiteboardListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let caller = caller(principal);
    let project_id = match arguments.project_id.as_deref() {
        None => None,
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("projectId", "Pass a project ID or link."),
        },
    };
    let organization_id = match arguments.workspace_id.as_deref() {
        None => None,
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("workspaceId", "Pass a workspace ID."),
        },
    };
    let linked = match (
        arguments.linked_to_kind.as_deref(),
        arguments.linked_to_id.as_deref(),
    ) {
        (None, None) => None,
        (Some(kind), Some(id)) => {
            let Some(kind) = TargetKind::parse(kind.trim()) else {
                return ToolResult::invalid("linkedToKind", "Use task, project, or page.");
            };
            let Some(id) = parse_id(id) else {
                return ToolResult::invalid("linkedToId", "Pass an ID or link.");
            };
            Some((kind, id))
        }
        _ => {
            return ToolResult::invalid(
                "linkedToKind",
                "Pass linkedToKind and linkedToId together.",
            );
        }
    };
    let result = async {
        // Only boards where Whiteboards is on for the caller (AWD-75); the
        // tool gate takes it being on anywhere.
        let on_in = whiteboards_on(state, principal).await?;
        let boards = match linked {
            Some((kind, id)) => whiteboards::linked_to(&state.pool, &caller, kind, id)
                .await?
                .into_iter()
                .filter(|board| match board.project_id {
                    Some(project) => on_in.projects.contains(&project),
                    None => on_in.workspaces.contains(&board.organization_id),
                })
                .collect(),
            None => {
                whiteboards::list(
                    &state.pool,
                    &caller,
                    &ListFilter {
                        organization_id,
                        project_id,
                        query: arguments.query.clone(),
                        deleted: arguments.deleted.unwrap_or(false),
                        limit: i64::from(arguments.limit.unwrap_or(50)),
                        on_in: Some(on_in),
                    },
                )
                .await?
            }
        };
        Ok::<_, AppError>(WhiteboardListValue {
            whiteboards: boards.into_iter().map(|board| line(state, board)).collect(),
        })
    }
    .await;
    match result {
        Ok(value) => ToolResult::ok(value),
        Err(error) => refused(error, "whiteboard_list"),
    }
}

// ---------------------------------------------------------------------------
// whiteboard_read

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WhiteboardReadArguments {
    pub whiteboard_id: String,
    /// Only these elements.
    pub element_ids: Option<Vec<String>>,
    /// Also return the raw Excalidraw elements, for details the summaries
    /// leave out.
    pub raw: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WhiteboardValue {
    pub whiteboard: WhiteboardLine,
    pub can_edit: bool,
    /// Pass it to whiteboard_edit as expectedSceneSeq.
    pub scene_seq: i64,
    /// Every live element, in stacking order (back to front).
    pub elements: Vec<ElementSummary>,
    /// True when there were more elements than returned.
    pub truncated: bool,
    /// Where the drawing is: x, y, width and height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<[f64; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Vec<Value>>,
    pub links: Vec<LinkView>,
    /// Who changed it in the last few minutes.
    pub recent_editors: Vec<Editor>,
    /// Saved versions, newest first; whiteboard_update restoreRevision puts
    /// one back.
    pub revisions: Vec<RevisionSummary>,
}

pub(super) async fn whiteboard_read(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WhiteboardReadArguments,
) -> ToolResult<WhiteboardValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let Some(id) = whiteboard_id(&arguments.whiteboard_id) else {
        return ToolResult::invalid("whiteboardId", "Pass a whiteboard ID or link.");
    };
    let caller = caller(principal);
    match board_module_off(state, &caller, id).await {
        Ok(true) => return ToolResult::module_off("whiteboard_read", "Whiteboards"),
        Ok(false) => {}
        Err(error) => return refused(error, "whiteboard_read"),
    }
    match read(
        state,
        &caller,
        id,
        arguments.element_ids.as_deref(),
        arguments.raw.unwrap_or(false),
    )
    .await
    {
        Ok(value) => ToolResult::ok(value),
        Err(error) => refused(error, "whiteboard_read"),
    }
}

async fn read(
    state: &AppState,
    caller: &Caller,
    id: Uuid,
    only: Option<&[String]>,
    raw: bool,
) -> Result<WhiteboardValue, AppError> {
    let board = {
        let mut connection = state.pool.acquire().await?;
        whiteboards::access(&mut connection, caller, id, false).await?
    };
    let view = store::scene(&state.pool, id, None).await?;
    let elements = view
        .elements
        .into_iter()
        .filter_map(|value| match value {
            Value::Object(object) => Some(object),
            _ => None,
        })
        .collect::<Vec<_>>();
    let board_scene = Scene::new(elements, 0);
    let wanted = |element: &&scene::Object| {
        only.is_none_or(|ids| ids.iter().any(|wanted| wanted == scene::id(element)))
    };
    let live = board_scene.live().filter(wanted).collect::<Vec<_>>();
    let bounds = scene::Bounds::union(live.iter().map(|element| scene::bounds(element)))
        .map(|b| [b.x, b.y, b.width, b.height]);
    let truncated = live.len() > MAX_READ_ELEMENTS;
    let summaries = live
        .iter()
        .take(MAX_READ_ELEMENTS)
        .map(|element| scene::summarize(element, &board_scene))
        .collect();
    let raw = raw.then(|| {
        live.iter()
            .take(MAX_READ_ELEMENTS)
            .map(|element| Value::Object((*element).clone()))
            .collect()
    });
    Ok(WhiteboardValue {
        whiteboard: line(state, whiteboards::summary(&state.pool, id).await?),
        can_edit: board.can_edit && !board.deleted,
        scene_seq: view.scene_seq,
        elements: summaries,
        truncated,
        bounds,
        raw,
        links: whiteboards::links(&state.pool, caller, id).await?,
        recent_editors: view.recent_editors,
        revisions: store::revisions(&state.pool, id, 10).await?,
    })
}

// ---------------------------------------------------------------------------
// whiteboard_render

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WhiteboardRenderArguments {
    pub whiteboard_id: String,
    /// Draw only these elements (and their labels), cropped to them.
    pub element_ids: Option<Vec<String>>,
    /// png (default) or svg.
    pub format: Option<String>,
    /// For png: pixels per board unit, 0.25 to 3 (default 1). Large boards
    /// are scaled down to fit 4096 pixels.
    pub scale: Option<f64>,
    /// Dark background.
    pub dark: Option<bool>,
    /// Leave the background transparent.
    pub transparent: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RenderValue {
    pub format: String,
    /// The image's size in pixels (png) or board units (svg).
    pub width: f64,
    pub height: f64,
    /// The part of the board drawn: x, y, width and height.
    pub area: [f64; 4],
    pub scene_seq: i64,
    /// For svg: the drawing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub svg: Option<String>,
}

fn render_result(result: ToolResult<RenderValue>, png: Option<Vec<u8>>) -> CallToolResult {
    let json = serde_json::to_value(&result).unwrap_or(Value::Null);
    let mut content = Vec::new();
    if let Some(png) = png {
        content.push(ContentBlock::image(STANDARD.encode(png), "image/png"));
    }
    content.push(ContentBlock::text(json.to_string()));
    let mut call = CallToolResult::success(content);
    call.structured_content = Some(json);
    call
}

pub(super) async fn whiteboard_render(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WhiteboardRenderArguments,
) -> CallToolResult {
    if !principal.can_read() {
        return render_result(ToolResult::denied("This connection can't read."), None);
    }
    let Some(id) = whiteboard_id(&arguments.whiteboard_id) else {
        return render_result(
            ToolResult::invalid("whiteboardId", "Pass a whiteboard ID or link."),
            None,
        );
    };
    let format = arguments
        .format
        .as_deref()
        .map(str::trim)
        .map(str::to_lowercase)
        .unwrap_or_else(|| "png".to_owned());
    if format != "png" && format != "svg" {
        return render_result(ToolResult::invalid("format", "Use png or svg."), None);
    }
    let scale = arguments.scale.unwrap_or(1.0);
    if !scale.is_finite() || !(0.25..=3.0).contains(&scale) {
        return render_result(
            ToolResult::invalid("scale", "Use a scale from 0.25 to 3."),
            None,
        );
    }
    let caller = caller(principal);
    match board_module_off(state, &caller, id).await {
        Ok(true) => {
            return render_result(
                ToolResult::module_off("whiteboard_render", "Whiteboards"),
                None,
            );
        }
        Ok(false) => {}
        Err(error) => return render_result(refused(error, "whiteboard_render"), None),
    }
    let drawn = async {
        let mut connection = state.pool.acquire().await?;
        whiteboards::access(&mut connection, &caller, id, false).await?;
        drop(connection);
        let view = store::scene(&state.pool, id, None).await?;
        let elements = view
            .elements
            .into_iter()
            .filter_map(|value| match value {
                Value::Object(object) => Some(object),
                _ => None,
            })
            .collect::<Vec<_>>();
        let files = store::files_for(&state.pool, id, &elements).await?;
        Ok::<_, AppError>((view.scene_seq, elements, files))
    }
    .await;
    let (scene_seq, elements, files) = match drawn {
        Ok(drawn) => drawn,
        Err(error) => return render_result(refused(error, "whiteboard_render"), None),
    };
    let options = render::Options {
        only: arguments.element_ids.clone(),
        background: !arguments.transparent.unwrap_or(false),
        dark: arguments.dark.unwrap_or(false),
    };
    // Drawing a big board is CPU work; keep it off the async workers.
    let rendered = tokio::task::spawn_blocking(move || {
        let rendered = render::svg(&elements, &files, &options);
        let png = (format == "png").then(|| render::png(&rendered.svg, scale));
        (rendered, png, format)
    })
    .await;
    let Ok((rendered, png, format)) = rendered else {
        return render_result(ToolResult::unavailable(), None);
    };
    let area = [
        rendered.area.x,
        rendered.area.y,
        rendered.area.width,
        rendered.area.height,
    ];
    match png {
        Some(Ok((bytes, width, height))) => render_result(
            ToolResult::ok(RenderValue {
                format,
                width: f64::from(width),
                height: f64::from(height),
                area,
                scene_seq,
                svg: None,
            }),
            Some(bytes),
        ),
        Some(Err(message)) => render_result(ToolResult::invalid("whiteboardId", message), None),
        None => render_result(
            ToolResult::ok(RenderValue {
                format,
                width: rendered.width,
                height: rendered.height,
                area,
                scene_seq,
                svg: Some(rendered.svg),
            }),
            None,
        ),
    }
}

// ---------------------------------------------------------------------------
// whiteboard_create

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LinkArgument {
    /// task, project, or page.
    pub target_kind: String,
    pub target_id: String,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WhiteboardCreateArguments {
    pub title: String,
    /// Put the board in this project: only its members see it. Left out,
    /// the board is for everyone in the workspace.
    pub project_id: Option<String>,
    /// The workspace (organization ID) for a workspace board. Defaults to
    /// the workspace of the project this connection acts in.
    pub workspace_id: Option<String>,
    /// Link the new board to a task, project or page.
    pub link_to: Option<LinkArgument>,
    /// Draw something straight away: the same operations as whiteboard_edit.
    pub operations: Option<Vec<Operation>>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WhiteboardChangeValue {
    pub whiteboard: WhiteboardLine,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<EditOutcome>,
}

/// The target to link, or which field is wrong and why.
fn link_argument(
    argument: &LinkArgument,
) -> Result<(TargetKind, Uuid), (&'static str, &'static str)> {
    let kind = TargetKind::parse(argument.target_kind.trim())
        .ok_or(("targetKind", "Use task, project, or page."))?;
    let id = parse_id(&argument.target_id).ok_or(("targetId", "Pass an ID or link."))?;
    Ok((kind, id))
}

fn edit_refused<T>(error: EditError, operation: &'static str) -> ToolResult<T> {
    match error {
        EditError::Invalid(message) => ToolResult::invalid("operations", message),
        EditError::Conflict(message) => ToolResult::conflict(message),
        EditError::App(error) => refused(error, operation),
    }
}

pub(super) async fn whiteboard_create(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WhiteboardCreateArguments,
) -> ToolResult<WhiteboardChangeValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let caller = caller(principal);
    let placement = match (
        arguments.project_id.as_deref(),
        arguments.workspace_id.as_deref(),
    ) {
        (Some(_), Some(_)) => {
            return ToolResult::invalid("projectId", "Pass projectId or workspaceId, not both.");
        }
        (Some(value), None) => match parse_id(value) {
            Some(id) => Placement::Project(id),
            None => return ToolResult::invalid("projectId", "Pass a project ID or link."),
        },
        (None, Some(value)) => match parse_id(value) {
            Some(id) => Placement::Workspace(id),
            None => return ToolResult::invalid("workspaceId", "Pass a workspace ID."),
        },
        // An older project-scoped credential stays in its project.
        (None, None) if caller.confined_project.is_some() => {
            Placement::Project(principal.project_id)
        }
        (None, None) => {
            match sqlx::query_scalar::<_, Uuid>(
                "SELECT organization_id FROM projects WHERE id = $1",
            )
            .bind(principal.project_id)
            .fetch_optional(&state.pool)
            .await
            {
                Ok(Some(organization_id)) => Placement::Workspace(organization_id),
                Ok(None) => {
                    return ToolResult::invalid("workspaceId", "Pass the workspace to put it in.");
                }
                Err(error) => return ToolResult::from_error(error.into(), "whiteboard_create"),
            }
        }
    };
    let link = match arguments.link_to.as_ref().map(link_argument).transpose() {
        Ok(link) => link,
        Err((field, message)) => return ToolResult::invalid(field, message),
    };
    // AWD-75: a board across a workspace needs Whiteboards on in that
    // workspace; the gate saw a project, or every workspace, not this one.
    // Only for someone create would let in (an active member, not confined
    // to a project); anyone else gets create's own refusal.
    if let Placement::Workspace(organization_id) = placement
        && caller.confined_project.is_none()
    {
        let modules: Result<_, AppError> = async {
            let mut connection = state.pool.acquire().await?;
            crate::modules::state::by_workspace(&mut connection, caller.actor_id).await
        }
        .await;
        match modules {
            Ok(modules) => {
                if modules
                    .get(&organization_id)
                    .is_some_and(|set| !set.contains("whiteboards"))
                {
                    return ToolResult::module_off("whiteboard_create", "Whiteboards");
                }
            }
            Err(error) => return refused(error, "whiteboard_create"),
        }
    }
    let result = async {
        let mut transaction = state.pool.begin().await?;
        let id =
            whiteboards::create(&mut transaction, &caller, placement, &arguments.title).await?;
        if let Some((kind, target_id)) = link {
            whiteboards::set_link(&mut transaction, &caller, id, kind, target_id, true).await?;
        }
        let edit = match arguments.operations.as_deref() {
            Some(operations) if !operations.is_empty() => {
                Some(edit::apply(&mut transaction, &caller, id, operations, None, false).await?)
            }
            _ => None,
        };
        transaction.commit().await?;
        Ok::<_, EditError>((id, edit))
    }
    .await;
    match result {
        Ok((id, edit)) => match whiteboards::summary(&state.pool, id).await {
            Ok(summary) => ToolResult::ok(WhiteboardChangeValue {
                whiteboard: line(state, summary),
                edit,
            }),
            Err(error) => refused(error, "whiteboard_create"),
        },
        Err(error) => edit_refused(error, "whiteboard_create"),
    }
}

// ---------------------------------------------------------------------------
// whiteboard_edit

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WhiteboardEditArguments {
    pub whiteboard_id: String,
    /// Applied in order, all or nothing. Each is {op, ...}:
    /// add {type, id?, x?, y?, width?, height?, text?, styles…} draws a
    /// shape (rectangle, ellipse, diamond, frame), text, line or arrow;
    /// connect {from, to, text?, id?} draws an arrow bound to both;
    /// update {id, text?, x?, y?, width?, height?, styles…};
    /// delete {ids} (labels and bound arrows go with their shape);
    /// move {ids, dx, dy}; align {ids, edge}; distribute {ids, axis, gap?};
    /// layout {ids?, direction?, gap?}; group/ungroup {ids};
    /// order {ids, position}. Arrows follow the shapes they connect.
    pub operations: Vec<Operation>,
    /// The sceneSeq you last read. When given, an operation naming an
    /// element someone else changed since is refused instead of applied.
    pub expected_scene_seq: Option<i64>,
    /// Needed to remove more than 25 elements at once. The board is saved
    /// first, so whiteboard_update restoreRevision can undo it.
    pub confirm_delete: Option<bool>,
}

pub(super) async fn whiteboard_edit(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WhiteboardEditArguments,
) -> ToolResult<EditOutcome> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(id) = whiteboard_id(&arguments.whiteboard_id) else {
        return ToolResult::invalid("whiteboardId", "Pass a whiteboard ID or link.");
    };
    let caller = caller(principal);
    match board_module_off(state, &caller, id).await {
        Ok(true) => return ToolResult::module_off("whiteboard_edit", "Whiteboards"),
        Ok(false) => {}
        Err(error) => return refused(error, "whiteboard_edit"),
    }
    let result = async {
        let mut transaction = state.pool.begin().await?;
        let board = whiteboards::access(&mut transaction, &caller, id, true).await?;
        if board.deleted {
            return Err(EditError::Conflict(
                "That whiteboard is in the trash. Restore it with whiteboard_update deleted: false first."
                    .to_owned(),
            ));
        }
        let outcome = edit::apply(
            &mut transaction,
            &caller,
            id,
            &arguments.operations,
            arguments.expected_scene_seq,
            arguments.confirm_delete.unwrap_or(false),
        )
        .await?;
        if !outcome.changed.is_empty() {
            whiteboards::audit(
                &mut transaction,
                &caller,
                board.organization_id,
                board.project_id,
                "whiteboard.edited",
                id,
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(outcome)
    }
    .await;
    match result {
        Ok(outcome) => ToolResult::ok(outcome),
        Err(error) => edit_refused(error, "whiteboard_edit"),
    }
}

// ---------------------------------------------------------------------------
// whiteboard_update

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WhiteboardUpdateArguments {
    pub whiteboard_id: String,
    pub title: Option<String>,
    /// Move the board into this project (only its members see it).
    pub project_id: Option<String>,
    /// Make the board visible to the whole workspace instead of one project.
    pub workspace_wide: Option<bool>,
    /// Move the board to the trash (true) or bring it back (false). Its
    /// drawing and links are kept either way.
    pub deleted: Option<bool>,
    /// Put the drawing back the way this saved revision had it. The
    /// current drawing is saved first.
    pub restore_revision: Option<i32>,
    /// The board version you last read; refuses the change if it moved on.
    pub expected_version: Option<i64>,
}

pub(super) async fn whiteboard_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WhiteboardUpdateArguments,
) -> ToolResult<WhiteboardChangeValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(id) = whiteboard_id(&arguments.whiteboard_id) else {
        return ToolResult::invalid("whiteboardId", "Pass a whiteboard ID or link.");
    };
    let project_id = match (arguments.project_id.as_deref(), arguments.workspace_wide) {
        (Some(_), Some(true)) => {
            return ToolResult::invalid(
                "workspaceWide",
                "Pass projectId or workspaceWide, not both.",
            );
        }
        (Some(value), _) => match parse_id(value) {
            Some(project) => Some(Some(project)),
            None => return ToolResult::invalid("projectId", "Pass a project ID or link."),
        },
        (None, Some(true)) => Some(None),
        (None, _) => None,
    };
    let change = BoardChange {
        title: arguments.title.clone(),
        project_id,
        deleted: arguments.deleted,
        expected_version: arguments.expected_version,
    };
    let caller = caller(principal);
    match board_module_off(state, &caller, id).await {
        Ok(true) => return ToolResult::module_off("whiteboard_update", "Whiteboards"),
        Ok(false) => {}
        Err(error) => return refused(error, "whiteboard_update"),
    }
    let result = async {
        let mut transaction = state.pool.begin().await?;
        whiteboards::change(&mut transaction, &caller, id, &change).await?;
        if let Some(revision) = arguments.restore_revision {
            let board = whiteboards::access(&mut transaction, &caller, id, true).await?;
            store::restore_revision(&mut transaction, &caller, id, revision).await?;
            whiteboards::audit(
                &mut transaction,
                &caller,
                board.organization_id,
                board.project_id,
                "whiteboard.revision_restored",
                id,
            )
            .await?;
        }
        transaction.commit().await?;
        whiteboards::summary(&state.pool, id).await
    }
    .await;
    match result {
        Ok(summary) => ToolResult::ok(WhiteboardChangeValue {
            whiteboard: line(state, summary),
            edit: None,
        }),
        Err(AppError::NotFound("whiteboard revision")) => ToolResult::not_found("That revision"),
        Err(error) => refused(error, "whiteboard_update"),
    }
}

// ---------------------------------------------------------------------------
// whiteboard_trash_empty

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WhiteboardTrashEmptyArguments {
    /// Only the trash of this project.
    pub project_id: Option<String>,
    /// Only the trash of this workspace (organization ID).
    pub workspace_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WhiteboardTrashEmptyValue {
    /// How many boards were deleted for good. Up to 200 go at a time; call
    /// again while this is 200.
    pub deleted: usize,
}

/// Empty the trash, as the web's "Empty trash" does: every trashed board the
/// caller may change, in the scope asked for and where Whiteboards is on
/// for them, goes for good.
pub(super) async fn whiteboard_trash_empty(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WhiteboardTrashEmptyArguments,
) -> ToolResult<WhiteboardTrashEmptyValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let project_id = match arguments.project_id.as_deref().map(parse_id) {
        None => None,
        Some(Some(id)) => Some(id),
        Some(None) => return ToolResult::invalid("projectId", "Pass a project ID or link."),
    };
    let organization_id = match arguments.workspace_id.as_deref().map(parse_id) {
        None => None,
        Some(Some(id)) => Some(id),
        Some(None) => return ToolResult::invalid("workspaceId", "Pass a workspace ID."),
    };
    let caller = caller(principal);
    let result = async {
        // Only where Whiteboards is on for the caller, as whiteboard_list
        // shows: a trashed board elsewhere stays.
        let on = whiteboards_on(state, principal).await?;
        let mut transaction = state.pool.begin().await?;
        let deleted = whiteboards::empty_trash(
            &mut transaction,
            &caller,
            organization_id,
            project_id,
            Some(&on),
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(deleted)
    }
    .await;
    match result {
        Ok(deleted) => ToolResult::ok(WhiteboardTrashEmptyValue { deleted }),
        Err(error) => refused(error, "whiteboard_trash_empty"),
    }
}

// ---------------------------------------------------------------------------
// whiteboard_duplicate

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WhiteboardDuplicateArguments {
    pub whiteboard_id: String,
    /// The copy's title. Default: "Copy of …".
    pub title: Option<String>,
}

pub(super) async fn whiteboard_duplicate(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WhiteboardDuplicateArguments,
) -> ToolResult<WhiteboardChangeValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(id) = whiteboard_id(&arguments.whiteboard_id) else {
        return ToolResult::invalid("whiteboardId", "Pass a whiteboard ID or link.");
    };
    let caller = caller(principal);
    match board_module_off(state, &caller, id).await {
        Ok(true) => return ToolResult::module_off("whiteboard_duplicate", "Whiteboards"),
        Ok(false) => {}
        Err(error) => return refused(error, "whiteboard_duplicate"),
    }
    let result = async {
        let mut transaction = state.pool.begin().await?;
        let copy =
            whiteboards::duplicate(&mut transaction, &caller, id, arguments.title.as_deref())
                .await?;
        transaction.commit().await?;
        whiteboards::summary(&state.pool, copy).await
    }
    .await;
    match result {
        Ok(summary) => ToolResult::ok(WhiteboardChangeValue {
            whiteboard: line(state, summary),
            edit: None,
        }),
        Err(error) => refused(error, "whiteboard_duplicate"),
    }
}

// ---------------------------------------------------------------------------
// whiteboard_link

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WhiteboardLinkArguments {
    pub whiteboard_id: String,
    /// task, project, or page.
    pub target_kind: String,
    /// The task (ID or link), project or page.
    pub target_id: String,
    /// Take the link off instead. The board itself stays.
    pub remove: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WhiteboardLinkValue {
    pub whiteboard_id: String,
    pub linked: bool,
    /// False when it was already that way.
    pub changed: bool,
    pub links: Vec<LinkView>,
}

pub(super) async fn whiteboard_link(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: WhiteboardLinkArguments,
) -> ToolResult<WhiteboardLinkValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let Some(id) = whiteboard_id(&arguments.whiteboard_id) else {
        return ToolResult::invalid("whiteboardId", "Pass a whiteboard ID or link.");
    };
    let Some(kind) = TargetKind::parse(arguments.target_kind.trim()) else {
        return ToolResult::invalid("targetKind", "Use task, project, or page.");
    };
    let Some(target_id) = parse_id(&arguments.target_id) else {
        return ToolResult::invalid("targetId", "Pass an ID or link.");
    };
    let linked = !arguments.remove.unwrap_or(false);
    match board_module_off(state, &caller(principal), id).await {
        Ok(true) => return ToolResult::module_off("whiteboard_link", "Whiteboards"),
        Ok(false) => {}
        Err(error) => return refused(error, "whiteboard_link"),
    }
    match set_link(state, principal, id, kind, target_id, linked).await {
        Ok(value) => ToolResult::ok(value),
        Err(error) => refused(error, "whiteboard_link"),
    }
}

/// Links or unlinks a board, for whiteboard_link and task_link; both check
/// board_module_off first (AWD-75).
pub(super) async fn set_link(
    state: &AppState,
    principal: &McpPrincipal,
    whiteboard_id: Uuid,
    kind: TargetKind,
    target_id: Uuid,
    linked: bool,
) -> Result<WhiteboardLinkValue, AppError> {
    let caller = caller(principal);
    let mut transaction = state.pool.begin().await?;
    let changed = whiteboards::set_link(
        &mut transaction,
        &caller,
        whiteboard_id,
        kind,
        target_id,
        linked,
    )
    .await?;
    transaction.commit().await?;
    Ok(WhiteboardLinkValue {
        whiteboard_id: whiteboard_id.to_string(),
        linked,
        changed,
        links: whiteboards::links(&state.pool, &caller, whiteboard_id).await?,
    })
}

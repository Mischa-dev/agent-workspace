//! The web app's routes for sheets (AWD-54 slice 2). Everyone in a project
//! reads its sheets; `sheets.edit` creates and changes them. Agents use the
//! tools in `mcp::sheet_tools`, over the same code (`sheets::apply`).
//!
//! Every write takes the project lock first, then the sheet's row, and is
//! idempotent by its `Idempotency-Key`. Open screens hear `sheet.updated`
//! with the new revision and read the tab they show again over these routes.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json_, json};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    Change, ChangeOutcome, CreateOutcome, ProblemKind, Sheet, SheetChange, SheetSummary,
    StoredCell, Tab,
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::parse_actor_kind,
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, IdempotencyReservation, RequestHash, complete, reserve,
        resource_bound_scope,
    },
    outbox,
    policy::Action,
    work_item_commands::{
        AuditAttribution, CommandActor, ProjectMembership, authorize_mutation, lock_membership,
        read_membership, record_audit_for,
    },
};

/// Changed cells a write sends back; the screen refetches past this.
const MAX_ECHOED_CELLS: usize = 2_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/sheets",
            get(list_sheets).post(create_sheet),
        )
        .route(
            "/api/v1/projects/{project_id}/sheets/{sheet_id}",
            get(get_sheet).patch(update_sheet),
        )
        .route(
            "/api/v1/projects/{project_id}/sheets/{sheet_id}/changes",
            post(change_sheet),
        )
}

// ---------------------------------------------------------------------------
// Access

fn browser_actor(
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<CommandActor, AppError> {
    let request_id = headers
        .get(crate::REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128)
        .map(str::to_owned);
    Ok(CommandActor {
        actor_id: principal.actor_id,
        actor_kind: parse_actor_kind(&principal.actor_kind)?,
        project_id,
        audit: AuditAttribution::Browser {
            session_id: principal.session_id,
            request_id,
        },
    })
}

fn allows(
    actor: &CommandActor,
    membership: &ProjectMembership,
    action: Action,
) -> Result<bool, AppError> {
    Ok(membership.enabled && authorize_mutation(actor, membership, action, None)?.is_allowed())
}

/// Whether the project can hold sheets: the Messages space can't.
async fn holds_sheets(connection: &mut PgConnection, project_id: Uuid) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT kind FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_optional(&mut *connection)
            .await?
            .is_some_and(|kind| kind != "direct"),
    )
}

/// Whether someone who can see the project may change its sheets now.
async fn reader(
    connection: &mut PgConnection,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<bool, AppError> {
    let membership = read_membership(connection, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    let actor = browser_actor(headers, principal, project_id)?;
    if !allows(&actor, &membership, Action::ProjectView)? {
        return Err(AppError::NotFound("project"));
    }
    if !holds_sheets(connection, project_id).await? {
        return Err(AppError::NotFound("project"));
    }
    Ok(allows(&actor, &membership, Action::SheetEdit)? && membership.project_archived_at.is_none())
}

/// A write that holds the project lock and an idempotency reservation.
struct Write {
    transaction: Transaction<'static, Postgres>,
    reservation: IdempotencyReservation,
    actor: CommandActor,
}

impl Write {
    async fn finish<T: Serialize>(
        mut self,
        status: StatusCode,
        body: &T,
    ) -> Result<Response, AppError> {
        complete(&mut self.transaction, &self.reservation, status, body).await?;
        self.transaction.commit().await?;
        Ok((status, Json(body)).into_response())
    }

    /// The sheet's row, locked after the project lock.
    async fn sheet(&mut self, sheet_id: Uuid) -> Result<Sheet, AppError> {
        super::lock(&mut self.transaction, self.actor.project_id, sheet_id)
            .await?
            .ok_or(AppError::NotFound("sheet"))
    }
}

enum Started {
    Replay(Response),
    Write(Box<Write>),
}

/// Checks the session, locks the project, authorizes `sheets.edit` (someone
/// who can't see the project gets not found, someone who can but may not
/// change sheets gets forbidden) and reserves the idempotency key. A
/// retried request gets the first response back.
async fn start_write<T: Serialize>(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    scope: String,
    request: &T,
) -> Result<Started, AppError> {
    require_authenticated_write(state, headers, principal)?;
    let key = IdempotencyKey::from_headers(headers)?;
    let request_hash = RequestHash::for_value(request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    let actor = browser_actor(headers, principal, project_id)?;
    if !allows(&actor, &membership, Action::ProjectView)? {
        return Err(AppError::NotFound("project"));
    }
    if !holds_sheets(&mut transaction, project_id).await? {
        return Err(AppError::NotFound("project"));
    }
    if !allows(&actor, &membership, Action::SheetEdit)? {
        return Err(AppError::Forbidden);
    }
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let body: Json_ = replay.decode()?;
            transaction.commit().await?;
            Ok(Started::Replay(
                (replay.status(), Json(body)).into_response(),
            ))
        }
        IdempotencyOutcome::Reserved(reservation) => Ok(Started::Write(Box::new(Write {
            transaction,
            reservation,
            actor,
        }))),
    }
}

macro_rules! begin {
    ($started:expr) => {
        match $started? {
            Started::Replay(response) => return Ok(response),
            Started::Write(write) => *write,
        }
    };
}

/// An error with a message written for the person, and optional details.
fn refusal(status: StatusCode, code: &str, message: impl Into<String>, details: Json_) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message.into(), "details": details } })),
    )
        .into_response()
}

fn invalid(field: &str, message: impl Into<String>) -> Response {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation_failed",
        message,
        json!({ "field": field }),
    )
}

fn name_message() -> String {
    format!(
        "Give it a name of 1 to {} characters, on one line.",
        super::MAX_NAME_CHARACTERS
    )
}

fn description_message() -> String {
    format!(
        "The description is one line of at most {} characters.",
        super::MAX_DESCRIPTION_CHARACTERS
    )
}

fn full_message() -> String {
    format!(
        "This project has as many active sheets as it can keep ({}). Archive one first.",
        super::MAX_ACTIVE_SHEETS
    )
}

// ---------------------------------------------------------------------------
// Bodies

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TabBody {
    id: Uuid,
    name: String,
    position: i32,
    structure_revision: i64,
    frozen_rows: i32,
    /// `{"<column number>": width}` for columns not at the default width.
    column_widths: Json_,
    /// The last row and column holding something (0 when it's empty).
    rows: i32,
    columns: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CellBody {
    row: i32,
    column: i32,
    input: String,
    kind: String,
    value: Json_,
    revision: i64,
}

impl From<StoredCell> for CellBody {
    fn from(cell: StoredCell) -> Self {
        Self {
            row: cell.row_number,
            column: cell.column_number,
            input: cell.input,
            kind: cell.kind,
            value: cell.value,
            revision: cell.revision,
        }
    }
}

/// A sheet as its screen reads it: the sheet, its tabs, and every cell of
/// one tab (a tab holds at most `MAX_CELLS`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SheetBody {
    sheet: SheetSummary,
    tabs: Vec<TabBody>,
    tab: Uuid,
    cells: Vec<CellBody>,
    can_edit: bool,
}

async fn sheet_body(
    connection: &mut PgConnection,
    sheet: &Sheet,
    tab: Option<&str>,
    can_edit: bool,
) -> Result<SheetBody, AppError> {
    let tabs = super::tabs(&mut *connection, sheet.id).await?;
    let chosen = pick_tab(&tabs, tab).ok_or(AppError::NotFound("tab"))?;
    let chosen_id = chosen.id;
    let extents = sqlx::query_as::<_, (Uuid, i32, i32, i64)>(
        "SELECT tab_id, max(row_number), max(column_number), count(*) \
         FROM sheet_cells WHERE sheet_id = $1 GROUP BY tab_id",
    )
    .bind(sheet.id)
    .fetch_all(&mut *connection)
    .await?;
    let cell_count = extents.iter().map(|(_, _, _, count)| count).sum();
    let cells = super::cells_in(
        &mut *connection,
        chosen_id,
        (1, super::MAX_ROWS),
        (1, super::MAX_COLUMNS),
        super::MAX_CELLS as i64,
    )
    .await?;
    Ok(SheetBody {
        sheet: SheetSummary {
            id: sheet.id.to_string(),
            name: sheet.name.clone(),
            description: sheet.description.clone(),
            tabs: tabs.iter().map(|tab| tab.name.clone()).collect(),
            cells: cell_count,
            revision: sheet.revision,
            archived: sheet.archived,
            created_at: sheet.created_at.clone(),
            updated_at: sheet.updated_at.clone(),
        },
        tabs: tabs
            .into_iter()
            .map(|tab| {
                let (rows, columns) = extents
                    .iter()
                    .find(|(id, ..)| *id == tab.id)
                    .map(|(_, rows, columns, _)| (*rows, *columns))
                    .unwrap_or((0, 0));
                TabBody {
                    id: tab.id,
                    name: tab.name,
                    position: tab.position,
                    structure_revision: tab.structure_revision,
                    frozen_rows: tab.frozen_rows,
                    column_widths: tab.column_widths,
                    rows,
                    columns,
                }
            })
            .collect(),
        tab: chosen_id,
        cells: cells.into_iter().map(CellBody::from).collect(),
        can_edit: can_edit && !sheet.archived,
    })
}

/// A tab by ID or name (ignoring case), or the first one.
fn pick_tab<'a>(tabs: &'a [Tab], reference: Option<&str>) -> Option<&'a Tab> {
    match reference.map(str::trim).filter(|value| !value.is_empty()) {
        None => tabs.first(),
        Some(reference) => {
            let id = reference.parse::<Uuid>().ok();
            tabs.iter()
                .find(|tab| Some(tab.id) == id || tab.name.eq_ignore_ascii_case(reference))
        }
    }
}

// ---------------------------------------------------------------------------
// Sheets

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    include_archived: Option<bool>,
}

async fn list_sheets(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<Json_>, AppError> {
    let mut connection = state.pool.acquire().await?;
    let can_edit = reader(&mut connection, &headers, &principal, project_id).await?;
    let sheets = super::list(
        &mut *connection,
        project_id,
        query.include_archived.unwrap_or(false),
    )
    .await?;
    Ok(Json(json!({ "sheets": sheets, "canEdit": can_edit })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SheetQuery {
    /// The tab whose cells come back, by ID or name; the first by default.
    tab: Option<String>,
}

async fn get_sheet(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, sheet_id)): ApiPath<(Uuid, Uuid)>,
    ApiQuery(query): ApiQuery<SheetQuery>,
) -> Result<Json<SheetBody>, AppError> {
    let mut connection = state.pool.acquire().await?;
    let can_edit = reader(&mut connection, &headers, &principal, project_id).await?;
    let sheet = super::find(&mut *connection, project_id, &sheet_id.to_string())
        .await?
        .filter(|sheet| sheet.id == sheet_id)
        .ok_or(AppError::NotFound("sheet"))?;
    Ok(Json(
        sheet_body(&mut connection, &sheet, query.tab.as_deref(), can_edit).await?,
    ))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateSheetRequest {
    name: String,
    #[serde(default)]
    description: String,
}

async fn create_sheet(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateSheetRequest>,
) -> Result<Response, AppError> {
    let Some(name) = super::normalize_name(&request.name) else {
        return Ok(invalid("name", name_message()));
    };
    let Some(description) = super::normalize_description(&request.description) else {
        return Ok(invalid("description", description_message()));
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            resource_bound_scope("projects", project_id, "sheets_create")?,
            &request,
        )
        .await
    );
    let sheet = match super::create(
        &mut write.transaction,
        project_id,
        principal.actor_id,
        &name,
        &description,
        super::FIRST_TAB_NAME,
    )
    .await?
    {
        CreateOutcome::Created(sheet) => sheet,
        CreateOutcome::NameTaken(sheet) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "sheet_name_taken",
                format!("A sheet is already called \"{}\".", sheet.name),
                json!({ "field": "name" }),
            ));
        }
        CreateOutcome::ProjectFull => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "sheet_limit",
                full_message(),
                Json_::Null,
            ));
        }
    };
    record_audit_for(
        &mut write.transaction,
        &write.actor,
        "sheet",
        "sheet.created",
        sheet.id,
        json!({ "revision": sheet.revision, "cells": 0 }),
    )
    .await?;
    outbox::enqueue_sheet_updated(&mut write.transaction, project_id, sheet.id, sheet.revision)
        .await?;
    let body = sheet_body(&mut write.transaction, &sheet, None, true).await?;
    write.finish(StatusCode::CREATED, &body).await
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateSheetRequest {
    name: Option<String>,
    description: Option<String>,
    archived: Option<bool>,
    /// The tab whose cells come back.
    tab: Option<String>,
}

async fn update_sheet(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, sheet_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateSheetRequest>,
) -> Result<Response, AppError> {
    let name = match request.name.as_deref() {
        Some(value) => match super::normalize_name(value) {
            Some(name) => Some(name),
            None => return Ok(invalid("name", name_message())),
        },
        None => None,
    };
    let description = match request.description.as_deref() {
        Some(value) => match super::normalize_description(value) {
            Some(description) => Some(description),
            None => return Ok(invalid("description", description_message())),
        },
        None => None,
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            resource_bound_scope("sheets", sheet_id, "update")?,
            &request,
        )
        .await
    );
    let sheet = write.sheet(sheet_id).await?;
    let change = SheetChange {
        name,
        description,
        archived: request.archived,
    };
    let sheet =
        match super::change(&mut write.transaction, &sheet, principal.actor_id, &change).await? {
            ChangeOutcome::Unchanged(sheet) => sheet,
            ChangeOutcome::Changed(sheet) => {
                record_audit_for(
                    &mut write.transaction,
                    &write.actor,
                    "sheet",
                    "sheet.changed",
                    sheet.id,
                    json!({ "revision": sheet.revision, "cells": 0 }),
                )
                .await?;
                outbox::enqueue_sheet_updated(
                    &mut write.transaction,
                    project_id,
                    sheet.id,
                    sheet.revision,
                )
                .await?;
                sheet
            }
            ChangeOutcome::NameTaken(taken) => {
                return Ok(refusal(
                    StatusCode::CONFLICT,
                    "sheet_name_taken",
                    format!("Another sheet here is called \"{}\".", taken.name),
                    json!({ "field": "name" }),
                ));
            }
            ChangeOutcome::ProjectFull => {
                return Ok(refusal(
                    StatusCode::CONFLICT,
                    "sheet_limit",
                    full_message(),
                    Json_::Null,
                ));
            }
        };
    let body = sheet_body(&mut write.transaction, &sheet, request.tab.as_deref(), true).await?;
    write.finish(StatusCode::OK, &body).await
}

// ---------------------------------------------------------------------------
// Cells and tabs

/// One change from the grid. Tabs are named by ID (or name); rows and
/// columns are numbers from 1.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum ChangeRequest {
    #[serde(rename_all = "camelCase")]
    Cell {
        tab: String,
        row: i32,
        column: i32,
        /// What was typed; empty clears the cell.
        input: String,
        /// The sheet revision the cell was read at.
        if_revision: Option<i64>,
    },
    #[serde(rename_all = "camelCase")]
    InsertRows { tab: String, row: i32, count: i32 },
    #[serde(rename_all = "camelCase")]
    DeleteRows { tab: String, row: i32, count: i32 },
    #[serde(rename_all = "camelCase")]
    InsertColumns {
        tab: String,
        column: i32,
        count: i32,
    },
    #[serde(rename_all = "camelCase")]
    DeleteColumns {
        tab: String,
        column: i32,
        count: i32,
    },
    #[serde(rename_all = "camelCase")]
    AddTab { name: String },
    #[serde(rename_all = "camelCase")]
    RenameTab { tab: String, name: String },
    #[serde(rename_all = "camelCase")]
    RemoveTab { tab: String },
    #[serde(rename_all = "camelCase")]
    MoveTab { tab: String, position: i32 },
    #[serde(rename_all = "camelCase")]
    FreezeRows { tab: String, rows: i32 },
    #[serde(rename_all = "camelCase")]
    ColumnWidth {
        tab: String,
        column: i32,
        /// Pixels; null puts the column back to the default.
        width: Option<i32>,
    },
}

impl ChangeRequest {
    fn into_change(self) -> Change {
        match self {
            Self::Cell {
                tab,
                row,
                column,
                input,
                if_revision,
            } => Change::Cell {
                tab: Some(tab),
                row,
                column,
                input,
                if_revision,
            },
            Self::InsertRows { tab, row, count } => Change::InsertRows {
                tab: Some(tab),
                row,
                count,
            },
            Self::DeleteRows { tab, row, count } => Change::DeleteRows {
                tab: Some(tab),
                row,
                count,
            },
            Self::InsertColumns { tab, column, count } => Change::InsertColumns {
                tab: Some(tab),
                column,
                count,
            },
            Self::DeleteColumns { tab, column, count } => Change::DeleteColumns {
                tab: Some(tab),
                column,
                count,
            },
            Self::AddTab { name } => Change::AddTab { name },
            Self::RenameTab { tab, name } => Change::RenameTab {
                tab: Some(tab),
                name,
            },
            Self::RemoveTab { tab } => Change::RemoveTab { tab: Some(tab) },
            Self::MoveTab { tab, position } => Change::MoveTab {
                tab: Some(tab),
                position,
            },
            Self::FreezeRows { tab, rows } => Change::FreezeRows {
                tab: Some(tab),
                rows,
            },
            Self::ColumnWidth { tab, column, width } => Change::ColumnWidth {
                tab: Some(tab),
                column,
                width,
            },
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangeSheetRequest {
    changes: Vec<ChangeRequest>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangedCellBody {
    tab: String,
    row: i32,
    column: i32,
    input: Option<String>,
    kind: &'static str,
    value: Json_,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangeSheetBody {
    revision: i64,
    inputs_changed: usize,
    values_changed: usize,
    /// Tabs were added, removed, renamed, moved or resized, or rows and
    /// columns moved: read the sheet again rather than patching cells.
    tabs_changed: bool,
    /// The cells that changed, by tab name, when there were few enough.
    changed: Vec<ChangedCellBody>,
    /// More cells changed than `changed` holds.
    truncated: bool,
}

async fn change_sheet(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, sheet_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<ChangeSheetRequest>,
) -> Result<Response, AppError> {
    if request.changes.is_empty() {
        return Ok(invalid("changes", "Send at least one change."));
    }
    if request.changes.len() > super::MAX_CHANGES {
        return Ok(invalid(
            "changes",
            format!("Send at most {} changes at once.", super::MAX_CHANGES),
        ));
    }
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            resource_bound_scope("sheets", sheet_id, "changes")?,
            &request,
        )
        .await
    );
    let sheet = write.sheet(sheet_id).await?;
    if sheet.archived {
        return Ok(refusal(
            StatusCode::CONFLICT,
            "sheet_archived",
            format!("{} is archived. Restore it to change it.", sheet.name),
            Json_::Null,
        ));
    }
    let changes = request
        .changes
        .iter()
        .cloned()
        .map(ChangeRequest::into_change)
        .collect::<Vec<_>>();
    let applied = match super::apply(
        &mut write.transaction,
        &sheet,
        principal.actor_id,
        &changes,
        MAX_ECHOED_CELLS,
    )
    .await?
    {
        Ok(applied) => applied,
        Err(problem) => {
            let (status, code) = match problem.kind {
                ProblemKind::Invalid => (StatusCode::UNPROCESSABLE_ENTITY, "validation_failed"),
                ProblemKind::NotFound => (StatusCode::NOT_FOUND, "not_found"),
                ProblemKind::Conflict => (StatusCode::CONFLICT, "sheet_conflict"),
                ProblemKind::Full => (StatusCode::CONFLICT, "sheet_limit"),
            };
            return Ok(refusal(
                status,
                code,
                format!("Nothing was saved. {}", problem.message),
                json!({ "field": format!("changes[{}]", problem.index), "index": problem.index }),
            ));
        }
    };
    if applied.revision != sheet.revision {
        record_audit_for(
            &mut write.transaction,
            &write.actor,
            "sheet",
            "sheet.changed",
            sheet.id,
            json!({ "revision": applied.revision, "cells": applied.inputs_changed }),
        )
        .await?;
        outbox::enqueue_sheet_updated(
            &mut write.transaction,
            project_id,
            sheet.id,
            applied.revision,
        )
        .await?;
    }
    let body = ChangeSheetBody {
        revision: applied.revision,
        inputs_changed: applied.inputs_changed,
        values_changed: applied.values_changed,
        tabs_changed: applied.tabs_changed,
        truncated: applied.changed.len() < applied.inputs_changed + applied.values_changed,
        changed: applied
            .changed
            .into_iter()
            .map(|cell| ChangedCellBody {
                tab: cell.tab,
                row: cell.row,
                column: cell.column,
                input: cell.input,
                kind: cell.value.kind(),
                value: cell.value.to_json(),
            })
            .collect(),
    };
    write.finish(StatusCode::OK, &body).await
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    #[test]
    fn grid_changes_read_as_tagged_objects() {
        let request: ChangeSheetRequest = serde_json::from_value(json!({
            "changes": [
                {"type": "cell", "tab": "Sheet1", "row": 2, "column": 3, "input": "=A1", "ifRevision": 4},
                {"type": "insertRows", "tab": "Sheet1", "row": 2, "count": 1},
                {"type": "columnWidth", "tab": "Sheet1", "column": 1, "width": null},
            ]
        }))
        .expect("changes parse");
        assert!(matches!(
            request.changes[0].clone().into_change(),
            Change::Cell {
                row: 2,
                column: 3,
                if_revision: Some(4),
                ..
            }
        ));
        assert!(matches!(
            request.changes[2].clone().into_change(),
            Change::ColumnWidth { width: None, .. }
        ));
        let unknown = serde_json::from_value::<ChangeSheetRequest>(json!({
            "changes": [{"type": "cell", "tab": "Sheet1", "row": 1, "column": 1, "input": "", "extra": 1}]
        }));
        assert!(unknown.is_err());
    }

    #[test]
    fn a_tab_is_found_by_id_or_name() {
        let tab = |name: &str| Tab {
            id: Uuid::now_v7(),
            name: name.to_owned(),
            position: 0,
            structure_revision: 1,
            frozen_rows: 1,
            column_widths: Json_::Object(Map::new()),
        };
        let tabs = vec![tab("Summary"), tab("Data")];
        assert_eq!(
            pick_tab(&tabs, None).map(|tab| tab.name.as_str()),
            Some("Summary")
        );
        assert_eq!(
            pick_tab(&tabs, Some("data")).map(|tab| tab.name.as_str()),
            Some("Data")
        );
        let id = tabs[1].id.to_string();
        assert_eq!(
            pick_tab(&tabs, Some(&id)).map(|tab| tab.name.as_str()),
            Some("Data")
        );
        assert!(pick_tab(&tabs, Some("Nope")).is_none());
    }
}

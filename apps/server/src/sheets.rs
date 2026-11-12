//! Sheets (AWD-54): a project's spreadsheets, computed on the server
//! (docs/plans/sheets.md; migration 20261112193000).
//!
//! * A sheet is a workbook with one or more tabs. Its cells keep what was
//!   typed (`input`) and what that computes to (`kind` and `value`).
//! * [`engine`]: the formula engine, behind one trait. Every change runs the
//!   whole workbook through it, so formulas, row and column inserts and tab
//!   renames behave as in any spreadsheet.
//! * [`address`]: A1 cell and range names.
//! * [`apply`] is the one way cells and tabs change, for agents (the
//!   `sheet_write` tool) and, later, the web app. A cell edit can name the
//!   revision it was read at: it's refused only if that cell, or its tab's
//!   rows and columns, changed since. Edits to different cells never clash.
//!
//! Everyone in a project reads its sheets; `sheets.edit` changes them.
//! Archiving hides a sheet; nothing is deleted. Every write takes the
//! project row lock first (`work_item_commands::lock_membership`), then the
//! sheet's row.

pub mod address;
pub mod engine;

use std::collections::{BTreeMap, HashMap};

use rmcp::schemars;
use serde::Serialize;
use serde_json::{Value as Json, json};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::error::AppError;
use address::column_name;
use engine::{FormulaEngine, IronCalc, Operation, TabInput, Value};

/// Active sheets one project can have.
pub const MAX_ACTIVE_SHEETS: i64 = 100;
/// Every sheet a project keeps, archived ones too.
pub const MAX_SHEET_ROWS: i64 = 500;
pub const MAX_TABS: usize = 20;
pub const MAX_ROWS: i32 = 10_000;
pub const MAX_COLUMNS: i32 = 200;
/// Cells that hold something, across a sheet's tabs.
pub const MAX_CELLS: usize = 50_000;
pub const MAX_INPUT_CHARACTERS: usize = 10_000;
pub const MAX_NAME_CHARACTERS: usize = 80;
pub const MAX_TAB_NAME_CHARACTERS: usize = 31;
pub const MAX_DESCRIPTION_CHARACTERS: usize = 500;
/// Changes one write can carry.
pub const MAX_CHANGES: usize = 2_000;
pub const MAX_FROZEN_ROWS: i32 = 20;
pub const MIN_COLUMN_WIDTH: i32 = 40;
pub const MAX_COLUMN_WIDTH: i32 = 800;
/// The tab a new sheet starts with.
pub const FIRST_TAB_NAME: &str = "Sheet1";

// ---------------------------------------------------------------------------
// Names

/// An ID, or a link whose last part is one (such as `/projects/…/sheets/<id>`).
pub fn id_in(value: &str) -> Option<Uuid> {
    crate::collections::id_in(value)
}

fn collapse(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A sheet's name: one line of 1 to 80 characters, never shaped like an ID.
pub fn normalize_name(value: &str) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    let value = collapse(value);
    let length = value.chars().count();
    (length > 0 && length <= MAX_NAME_CHARACTERS && value.parse::<Uuid>().is_err()).then_some(value)
}

/// A tab's name, as spreadsheets allow: up to 31 characters, none of
/// `[]:*?/\`, and not starting or ending with an apostrophe.
pub fn normalize_tab_name(value: &str) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    let value = collapse(value);
    let length = value.chars().count();
    (length > 0
        && length <= MAX_TAB_NAME_CHARACTERS
        && !value.contains(['[', ']', ':', '*', '?', '/', '\\'])
        && !value.starts_with('\'')
        && !value.ends_with('\'')
        && value.parse::<Uuid>().is_err())
    .then_some(value)
}

/// What a sheet is for, on one line. Empty clears it.
pub fn normalize_description(value: &str) -> Option<String> {
    let value = value.trim();
    (!value
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
        && value.chars().count() <= MAX_DESCRIPTION_CHARACTERS)
        .then(|| value.to_owned())
}

// ---------------------------------------------------------------------------
// Rows

#[derive(Clone, Debug, FromRow)]
pub struct Sheet {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub description: String,
    pub revision: i64,
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
}

const COLUMNS: &str = "sheet.id, sheet.project_id, sheet.name, sheet.description, \
     sheet.revision, sheet.archived_at IS NOT NULL AS archived, \
     sheet.created_at::text AS created_at, sheet.updated_at::text AS updated_at";

#[derive(Clone, Debug, FromRow)]
pub struct Tab {
    pub id: Uuid,
    pub name: String,
    /// 0-based.
    pub position: i32,
    pub structure_revision: i64,
    pub frozen_rows: i32,
    pub column_widths: Json,
}

#[derive(Clone, Debug, FromRow)]
pub struct StoredCell {
    pub tab_id: Uuid,
    pub row_number: i32,
    pub column_number: i32,
    pub input: String,
    pub kind: String,
    pub value: Json,
    pub revision: i64,
}

impl StoredCell {
    pub fn value(&self) -> Value {
        Value::from_stored(&self.kind, &self.value)
    }
}

/// A sheet as the web app and agents see it in a list.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SheetSummary {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Its tabs' names, in order.
    pub tabs: Vec<String>,
    /// Cells that hold something.
    pub cells: i64,
    /// Goes up by one with every change; pass it back as ifRevision.
    pub revision: i64,
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(FromRow)]
struct SummaryRow {
    #[sqlx(flatten)]
    sheet: Sheet,
    tabs: Vec<String>,
    cells: i64,
}

impl From<SummaryRow> for SheetSummary {
    fn from(row: SummaryRow) -> Self {
        Self {
            id: row.sheet.id.to_string(),
            name: row.sheet.name,
            description: row.sheet.description,
            tabs: row.tabs,
            cells: row.cells,
            revision: row.sheet.revision,
            archived: row.sheet.archived,
            created_at: row.sheet.created_at,
            updated_at: row.sheet.updated_at,
        }
    }
}

/// A project's sheets, last changed first; archived ones when asked for.
pub async fn list(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    include_archived: bool,
) -> Result<Vec<SheetSummary>, AppError> {
    Ok(sqlx::query_as::<_, SummaryRow>(&format!(
        "SELECT {COLUMNS}, \
           ARRAY(SELECT tab.name FROM sheet_tabs tab WHERE tab.sheet_id = sheet.id \
                 ORDER BY tab.position, tab.id) AS tabs, \
           (SELECT count(*) FROM sheet_cells cell WHERE cell.sheet_id = sheet.id) AS cells \
         FROM sheets sheet \
         WHERE sheet.project_id = $1 AND ($2 OR sheet.archived_at IS NULL) \
         ORDER BY sheet.archived_at IS NOT NULL, sheet.updated_at DESC, sheet.id \
         LIMIT {MAX_SHEET_ROWS}"
    ))
    .bind(project_id)
    .bind(include_archived)
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(SheetSummary::from)
    .collect())
}

/// One of the project's sheets by ID or name (ignoring case). A name finds
/// the active sheet first, then the archived one archived last.
pub async fn find(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    reference: &str,
) -> Result<Option<Sheet>, AppError> {
    let reference = reference.trim();
    let id = id_in(reference);
    Ok(sqlx::query_as::<_, Sheet>(&format!(
        "SELECT {COLUMNS} FROM sheets sheet \
         WHERE sheet.project_id = $1 \
           AND (sheet.id = $2 OR ($2 IS NULL AND lower(sheet.name) = lower($3))) \
         ORDER BY sheet.archived_at IS NOT NULL, sheet.archived_at DESC, sheet.id \
         LIMIT 1"
    ))
    .bind(project_id)
    .bind(id)
    .bind(collapse(reference))
    .fetch_optional(executor)
    .await?)
}

/// Locks a sheet's row for a write. The caller holds the project lock.
pub async fn lock(
    connection: &mut PgConnection,
    project_id: Uuid,
    sheet_id: Uuid,
) -> Result<Option<Sheet>, AppError> {
    Ok(sqlx::query_as::<_, Sheet>(&format!(
        "SELECT {COLUMNS} FROM sheets sheet \
         WHERE sheet.project_id = $1 AND sheet.id = $2 FOR UPDATE"
    ))
    .bind(project_id)
    .bind(sheet_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The names of the project's active sheets, for messages.
pub async fn active_names(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
) -> Result<String, AppError> {
    let names = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sheets WHERE project_id = $1 AND archived_at IS NULL \
         ORDER BY updated_at DESC, id LIMIT 50",
    )
    .bind(project_id)
    .fetch_all(executor)
    .await?;
    Ok(if names.is_empty() {
        "none yet".to_owned()
    } else {
        names.join(", ")
    })
}

/// A sheet's tabs in order.
pub async fn tabs(
    executor: impl sqlx::PgExecutor<'_>,
    sheet_id: Uuid,
) -> Result<Vec<Tab>, AppError> {
    Ok(sqlx::query_as::<_, Tab>(
        "SELECT id, name, position, structure_revision, frozen_rows, column_widths \
         FROM sheet_tabs WHERE sheet_id = $1 ORDER BY position, id",
    )
    .bind(sheet_id)
    .fetch_all(executor)
    .await?)
}

/// A sheet's cells, by tab, row and column.
pub async fn cells(
    executor: impl sqlx::PgExecutor<'_>,
    sheet_id: Uuid,
) -> Result<Vec<StoredCell>, AppError> {
    Ok(sqlx::query_as::<_, StoredCell>(
        "SELECT tab_id, row_number, column_number, input, kind, value, revision \
         FROM sheet_cells WHERE sheet_id = $1 \
         ORDER BY tab_id, row_number, column_number",
    )
    .bind(sheet_id)
    .fetch_all(executor)
    .await?)
}

/// One tab's cells in a block of rows and columns (both inclusive).
pub async fn cells_in(
    executor: impl sqlx::PgExecutor<'_>,
    tab_id: Uuid,
    rows: (i32, i32),
    columns: (i32, i32),
    limit: i64,
) -> Result<Vec<StoredCell>, AppError> {
    Ok(sqlx::query_as::<_, StoredCell>(
        "SELECT tab_id, row_number, column_number, input, kind, value, revision \
         FROM sheet_cells \
         WHERE tab_id = $1 AND row_number BETWEEN $2 AND $3 \
           AND column_number BETWEEN $4 AND $5 \
         ORDER BY row_number, column_number LIMIT $6",
    )
    .bind(tab_id)
    .bind(rows.0)
    .bind(rows.1)
    .bind(columns.0)
    .bind(columns.1)
    .bind(limit)
    .fetch_all(executor)
    .await?)
}

/// The last row and column a tab uses, or (0, 0) when it's empty.
pub async fn used_extent(
    executor: impl sqlx::PgExecutor<'_>,
    tab_id: Uuid,
) -> Result<(i32, i32), AppError> {
    let (rows, columns) = sqlx::query_as::<_, (Option<i32>, Option<i32>)>(
        "SELECT max(row_number), max(column_number) FROM sheet_cells WHERE tab_id = $1",
    )
    .bind(tab_id)
    .fetch_one(executor)
    .await?;
    Ok((rows.unwrap_or(0), columns.unwrap_or(0)))
}

// ---------------------------------------------------------------------------
// Creating, renaming and archiving

#[derive(Clone, Debug)]
pub enum CreateOutcome {
    Created(Sheet),
    /// An active sheet already has the name, ignoring case.
    NameTaken(Sheet),
    /// `MAX_ACTIVE_SHEETS` active ones, or `MAX_SHEET_ROWS` in all.
    ProjectFull,
}

/// Creates a sheet with one empty tab. The caller holds the project lock and
/// has checked the actor may edit sheets.
pub async fn create(
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
    name: &str,
    description: &str,
    first_tab: &str,
) -> Result<CreateOutcome, AppError> {
    if let Some(existing) = find(&mut *connection, project_id, name).await?
        && !existing.archived
    {
        return Ok(CreateOutcome::NameTaken(existing));
    }
    let (active, all) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT count(*) FILTER (WHERE archived_at IS NULL), count(*) \
         FROM sheets WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut *connection)
    .await?;
    if active >= MAX_ACTIVE_SHEETS || all >= MAX_SHEET_ROWS {
        return Ok(CreateOutcome::ProjectFull);
    }
    let id = Uuid::now_v7();
    let sheet = sqlx::query_as::<_, Sheet>(&format!(
        "INSERT INTO sheets AS sheet \
           (id, project_id, name, description, created_by_actor_id, updated_by_actor_id) \
         VALUES ($1, $2, $3, $4, $5, $5) RETURNING {COLUMNS}"
    ))
    .bind(id)
    .bind(project_id)
    .bind(name)
    .bind(description)
    .bind(actor_id)
    .fetch_one(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO sheet_tabs (id, project_id, sheet_id, name, position) \
         VALUES ($1, $2, $3, $4, 0)",
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(id)
    .bind(first_tab)
    .execute(&mut *connection)
    .await?;
    insert_revision(
        &mut *connection,
        project_id,
        id,
        1,
        actor_id,
        json!({"created": true}),
    )
    .await?;
    Ok(CreateOutcome::Created(sheet))
}

#[derive(Clone, Debug, Default)]
pub struct SheetChange {
    pub name: Option<String>,
    pub description: Option<String>,
    pub archived: Option<bool>,
}

#[derive(Clone, Debug)]
pub enum ChangeOutcome {
    Changed(Sheet),
    Unchanged(Sheet),
    NameTaken(Sheet),
    ProjectFull,
}

/// Renames, describes, archives or restores a locked sheet.
pub async fn change(
    connection: &mut PgConnection,
    sheet: &Sheet,
    actor_id: Uuid,
    change: &SheetChange,
) -> Result<ChangeOutcome, AppError> {
    let name = change.name.clone().unwrap_or_else(|| sheet.name.clone());
    let description = change
        .description
        .clone()
        .unwrap_or_else(|| sheet.description.clone());
    let archived = change.archived.unwrap_or(sheet.archived);
    if name == sheet.name && description == sheet.description && archived == sheet.archived {
        return Ok(ChangeOutcome::Unchanged(sheet.clone()));
    }
    if !archived {
        let taken = sqlx::query_as::<_, Sheet>(&format!(
            "SELECT {COLUMNS} FROM sheets sheet \
             WHERE sheet.project_id = $1 AND sheet.id <> $2 AND sheet.archived_at IS NULL \
               AND lower(sheet.name) = lower($3) LIMIT 1"
        ))
        .bind(sheet.project_id)
        .bind(sheet.id)
        .bind(&name)
        .fetch_optional(&mut *connection)
        .await?;
        if let Some(taken) = taken {
            return Ok(ChangeOutcome::NameTaken(taken));
        }
        if sheet.archived {
            let active = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM sheets WHERE project_id = $1 AND archived_at IS NULL",
            )
            .bind(sheet.project_id)
            .fetch_one(&mut *connection)
            .await?;
            if active >= MAX_ACTIVE_SHEETS {
                return Ok(ChangeOutcome::ProjectFull);
            }
        }
    }
    let revision = sheet.revision + 1;
    let changed = sqlx::query_as::<_, Sheet>(&format!(
        "UPDATE sheets AS sheet SET name = $3, description = $4, \
           archived_at = CASE WHEN $5 THEN coalesce(sheet.archived_at, now()) END, \
           archived_by_actor_id = CASE WHEN $5 THEN coalesce(sheet.archived_by_actor_id, $6) END, \
           revision = $7, updated_by_actor_id = $6, updated_at = now() \
         WHERE sheet.project_id = $1 AND sheet.id = $2 RETURNING {COLUMNS}"
    ))
    .bind(sheet.project_id)
    .bind(sheet.id)
    .bind(&name)
    .bind(&description)
    .bind(archived)
    .bind(actor_id)
    .bind(revision)
    .fetch_one(&mut *connection)
    .await?;
    let mut summary = serde_json::Map::new();
    if name != sheet.name {
        summary.insert(
            "renamed".to_owned(),
            json!({"from": sheet.name, "to": name}),
        );
    }
    if description != sheet.description {
        summary.insert("described".to_owned(), json!(true));
    }
    if archived != sheet.archived {
        summary.insert("archived".to_owned(), json!(archived));
    }
    insert_revision(
        &mut *connection,
        sheet.project_id,
        sheet.id,
        revision,
        actor_id,
        Json::Object(summary),
    )
    .await?;
    Ok(ChangeOutcome::Changed(changed))
}

async fn insert_revision(
    connection: &mut PgConnection,
    project_id: Uuid,
    sheet_id: Uuid,
    revision: i64,
    actor_id: Uuid,
    summary: Json,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO sheet_revisions (id, project_id, sheet_id, revision, actor_id, summary) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(sheet_id)
    .bind(revision)
    .bind(actor_id)
    .bind(summary)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Changing cells and tabs

/// A tab named in a change: its name or ID, or the first tab when `None`.
pub type TabRef = Option<String>;

#[derive(Clone, Debug)]
pub enum Change {
    /// Set one cell's input; empty clears it. `if_revision` is the sheet
    /// revision the writer read the cell at.
    Cell {
        tab: TabRef,
        row: i32,
        column: i32,
        input: String,
        if_revision: Option<i64>,
    },
    /// Set the cells of a row found by its value in `key_column` (a header
    /// in row 1), or add it under the last row. Values by header; JSON null
    /// clears a cell. A header that doesn't exist yet is added to row 1.
    Row {
        tab: TabRef,
        key_column: Option<String>,
        values: Vec<(String, Json)>,
    },
    InsertRows {
        tab: TabRef,
        row: i32,
        count: i32,
    },
    DeleteRows {
        tab: TabRef,
        row: i32,
        count: i32,
    },
    InsertColumns {
        tab: TabRef,
        column: i32,
        count: i32,
    },
    DeleteColumns {
        tab: TabRef,
        column: i32,
        count: i32,
    },
    AddTab {
        name: String,
    },
    RenameTab {
        tab: TabRef,
        name: String,
    },
    RemoveTab {
        tab: TabRef,
    },
    /// Move a tab; 1 is first.
    MoveTab {
        tab: TabRef,
        position: i32,
    },
    FreezeRows {
        tab: TabRef,
        rows: i32,
    },
    ColumnWidth {
        tab: TabRef,
        column: i32,
        width: Option<i32>,
    },
}

impl Change {
    fn short(&self) -> &'static str {
        match self {
            Self::Cell { .. } => "cell",
            Self::Row { .. } => "row",
            Self::InsertRows { .. } => "insertRows",
            Self::DeleteRows { .. } => "deleteRows",
            Self::InsertColumns { .. } => "insertColumns",
            Self::DeleteColumns { .. } => "deleteColumns",
            Self::AddTab { .. } => "addTab",
            Self::RenameTab { .. } => "renameTab",
            Self::RemoveTab { .. } => "removeTab",
            Self::MoveTab { .. } => "moveTab",
            Self::FreezeRows { .. } => "freezeRows",
            Self::ColumnWidth { .. } => "columnWidth",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProblemKind {
    Invalid,
    NotFound,
    Conflict,
    Full,
}

/// Why a write was refused; nothing was written.
#[derive(Clone, Debug)]
pub struct Problem {
    /// The change's index in the list.
    pub index: usize,
    pub kind: ProblemKind,
    pub message: String,
}

fn problem(index: usize, kind: ProblemKind, message: impl Into<String>) -> Problem {
    Problem {
        index,
        kind,
        message: message.into(),
    }
}

/// A cell whose input or value a write changed.
#[derive(Clone, Debug)]
pub struct ChangedCell {
    pub tab: String,
    pub row: i32,
    pub column: i32,
    pub input: Option<String>,
    pub value: Value,
}

impl ChangedCell {
    pub fn address(&self) -> String {
        format!("{}{}", column_name(self.column), self.row)
    }
}

#[derive(Clone, Debug)]
pub struct Applied {
    /// The sheet's revision now (unchanged when nothing changed).
    pub revision: i64,
    /// Cells whose input changed (set, cleared or moved).
    pub inputs_changed: usize,
    /// Cells whose value changed because something they use changed.
    pub values_changed: usize,
    pub tabs_changed: bool,
    /// Up to `limit` of the cells that changed, inputs first.
    pub changed: Vec<ChangedCell>,
}

/// A tab as a write sees it while its changes are worked through.
#[derive(Clone, Debug)]
struct WorkingTab {
    /// `None` for a tab this write adds.
    id: Option<Uuid>,
    name: String,
    stored_structure_revision: i64,
    renamed: bool,
    structure_changed: bool,
    frozen_rows: i32,
    column_widths: serde_json::Map<String, Json>,
    settings_changed: bool,
    /// Rows and columns this write inserted or deleted, in order.
    moves: Vec<Move>,
}

/// Rows (or columns) inserted or deleted at `at`.
#[derive(Clone, Copy, Debug)]
struct Move {
    rows: bool,
    insert: bool,
    at: i32,
    count: i32,
}

impl WorkingTab {
    /// Where a cell this write names was before its inserts and deletes,
    /// or `None` for one an insert made.
    fn original(&self, row: i32, column: i32) -> Option<(i32, i32)> {
        let (mut row, mut column) = (row, column);
        for step in self.moves.iter().rev() {
            let place = if step.rows { &mut row } else { &mut column };
            if step.insert {
                if *place >= step.at + step.count {
                    *place -= step.count;
                } else if *place >= step.at {
                    return None;
                }
            } else if *place >= step.at {
                *place += step.count;
            }
        }
        Some((row, column))
    }
}

/// Two row keys are the same when their text is, or both are the same number.
fn same_key(candidate: &str, wanted: &str) -> bool {
    let candidate = candidate.trim().trim_start_matches('\'');
    if candidate == wanted {
        return true;
    }
    matches!(
        (candidate.parse::<f64>(), wanted.parse::<f64>()),
        (Ok(left), Ok(right)) if left == right
    )
}

fn resolve_tab(tabs: &[WorkingTab], reference: &TabRef) -> Option<usize> {
    match reference.as_deref().map(str::trim) {
        None | Some("") => (!tabs.is_empty()).then_some(0),
        Some(reference) => {
            let id = reference.parse::<Uuid>().ok();
            tabs.iter().position(|tab| {
                (id.is_some() && tab.id == id) || tab.name.eq_ignore_ascii_case(reference)
            })
        }
    }
}

fn tab_names(tabs: &[WorkingTab]) -> String {
    tabs.iter()
        .map(|tab| tab.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A JSON value as a cell's input: text as typed (`=` starts a formula),
/// numbers and booleans as a spreadsheet writes them, null clears.
pub fn input_of(value: &Json) -> Option<String> {
    match value {
        Json::Null => Some(String::new()),
        Json::String(text) => Some(text.clone()),
        Json::Bool(true) => Some("TRUE".to_owned()),
        Json::Bool(false) => Some("FALSE".to_owned()),
        Json::Number(number) => Some(number.to_string()),
        Json::Array(_) | Json::Object(_) => None,
    }
}

/// A value as text, for matching keys and headers.
pub fn text_of(value: &Value) -> String {
    match value {
        Value::Empty => String::new(),
        Value::Number(number) => {
            if number.fract() == 0.0 && number.abs() < 1e15 {
                format!("{}", *number as i64)
            } else {
                number.to_string()
            }
        }
        Value::Text(text) | Value::Error(text) => text.clone(),
        Value::Boolean(true) => "TRUE".to_owned(),
        Value::Boolean(false) => "FALSE".to_owned(),
    }
}

fn check_input(index: usize, input: &str) -> Result<(), Problem> {
    if input.chars().count() > MAX_INPUT_CHARACTERS {
        return Err(problem(
            index,
            ProblemKind::Invalid,
            format!("A cell holds up to {MAX_INPUT_CHARACTERS} characters."),
        ));
    }
    if input.contains('\0') {
        return Err(problem(
            index,
            ProblemKind::Invalid,
            "A cell can't hold a NUL character.",
        ));
    }
    Ok(())
}

fn check_row(index: usize, row: i32) -> Result<(), Problem> {
    if (1..=MAX_ROWS).contains(&row) {
        Ok(())
    } else {
        Err(problem(
            index,
            ProblemKind::Invalid,
            format!("Rows go from 1 to {MAX_ROWS}."),
        ))
    }
}

fn check_column(index: usize, column: i32) -> Result<(), Problem> {
    if (1..=MAX_COLUMNS).contains(&column) {
        Ok(())
    } else {
        Err(problem(
            index,
            ProblemKind::Invalid,
            format!(
                "Columns go from A to {} ({MAX_COLUMNS}).",
                column_name(MAX_COLUMNS)
            ),
        ))
    }
}

fn check_count(index: usize, count: i32, max: i32) -> Result<(), Problem> {
    if (1..=max).contains(&count) {
        Ok(())
    } else {
        Err(problem(
            index,
            ProblemKind::Invalid,
            format!("count goes from 1 to {max}."),
        ))
    }
}

/// Applies `changes` to a locked sheet, all or nothing, and recalculates it.
/// The caller holds the project lock and the sheet's row lock, and has
/// checked the actor may edit sheets. `limit` caps the changed cells
/// returned.
pub async fn apply(
    connection: &mut PgConnection,
    sheet: &Sheet,
    actor_id: Uuid,
    changes: &[Change],
    limit: usize,
) -> Result<Result<Applied, Problem>, AppError> {
    apply_with(connection, sheet, actor_id, changes, limit, &IronCalc).await
}

pub async fn apply_with(
    connection: &mut PgConnection,
    sheet: &Sheet,
    actor_id: Uuid,
    changes: &[Change],
    limit: usize,
    engine: &(impl FormulaEngine + Clone + 'static),
) -> Result<Result<Applied, Problem>, AppError> {
    if changes.len() > MAX_CHANGES {
        return Ok(Err(problem(
            0,
            ProblemKind::Invalid,
            format!("Send up to {MAX_CHANGES} changes at a time."),
        )));
    }
    let stored_tabs = tabs(&mut *connection, sheet.id).await?;
    let stored_cells = cells(&mut *connection, sheet.id).await?;
    let mut by_tab: HashMap<Uuid, Vec<&StoredCell>> = HashMap::new();
    for cell in &stored_cells {
        by_tab.entry(cell.tab_id).or_default().push(cell);
    }
    let stored_at: HashMap<(Uuid, i32, i32), &StoredCell> = stored_cells
        .iter()
        .map(|cell| ((cell.tab_id, cell.row_number, cell.column_number), cell))
        .collect();
    // Cells cleared since their tab's rows and columns last moved, and the
    // revision that cleared each.
    let cleared_at: HashMap<(Uuid, i32, i32), i64> = sqlx::query_as::<_, (Uuid, i32, i32, i64)>(
        "SELECT cleared.tab_id, cleared.row_number, cleared.column_number, cleared.revision \
             FROM sheet_cleared_cells cleared JOIN sheet_tabs tab ON tab.id = cleared.tab_id \
             WHERE tab.sheet_id = $1",
    )
    .bind(sheet.id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(tab, row, column, revision)| ((tab, row, column), revision))
    .collect();

    let mut working = stored_tabs
        .iter()
        .map(|tab| WorkingTab {
            id: Some(tab.id),
            name: tab.name.clone(),
            stored_structure_revision: tab.structure_revision,
            renamed: false,
            structure_changed: false,
            frozen_rows: tab.frozen_rows,
            column_widths: tab.column_widths.as_object().cloned().unwrap_or_default(),
            settings_changed: false,
            moves: Vec::new(),
        })
        .collect::<Vec<_>>();
    // Order kept apart from the engine's: moving a tab doesn't touch cells.
    let mut operations = Vec::new();
    // Rows a `Row` change added, per tab, so two in one write don't collide.
    let mut appended: HashMap<usize, i32> = HashMap::new();
    let mut moves: Vec<(usize, Uuid, i32)> = Vec::new();

    for (index, change) in changes.iter().enumerate() {
        let tab_at = |working: &[WorkingTab], reference: &TabRef| {
            resolve_tab(working, reference).ok_or_else(|| {
                problem(
                    index,
                    ProblemKind::NotFound,
                    format!(
                        "No tab called \"{}\" in \"{}\". Its tabs: {}.",
                        reference.as_deref().unwrap_or_default(),
                        sheet.name,
                        tab_names(working)
                    ),
                )
            })
        };
        macro_rules! tab {
            ($reference:expr) => {
                match tab_at(&working, $reference) {
                    Ok(tab) => tab,
                    Err(refusal) => return Ok(Err(refusal)),
                }
            };
        }
        macro_rules! check {
            ($check:expr) => {
                if let Err(refusal) = $check {
                    return Ok(Err(refusal));
                }
            };
        }
        match change {
            Change::Cell {
                tab,
                row,
                column,
                input,
                if_revision,
            } => {
                let tab = tab!(tab);
                check!(check_row(index, *row));
                check!(check_column(index, *column));
                check!(check_input(index, input));
                if let Some(read_at) = if_revision {
                    let working_tab = &working[tab];
                    if working_tab.stored_structure_revision > *read_at {
                        return Ok(Err(problem(
                            index,
                            ProblemKind::Conflict,
                            format!(
                                "Rows or columns of \"{}\" moved, or it was renamed, since revision {read_at}. Read the sheet again and retry.",
                                working_tab.name
                            ),
                        )));
                    }
                    // The cell where it was before this write's own inserts
                    // and deletes; one an insert made is new.
                    let before = working_tab
                        .id
                        .zip(working_tab.original(*row, *column))
                        .map(|(id, (row, column))| (id, row, column));
                    if let Some(at) = before
                        && let Some(cleared) = cleared_at.get(&at)
                        && *cleared > *read_at
                    {
                        return Ok(Err(problem(
                            index,
                            ProblemKind::Conflict,
                            format!(
                                "{}{} in \"{}\" was cleared at revision {cleared}, after revision {read_at}. Read it again and retry.",
                                column_name(*column),
                                row,
                                working_tab.name,
                            ),
                        )));
                    }
                    if let Some(at) = before
                        && let Some(stored) = stored_at.get(&at)
                        && stored.revision > *read_at
                    {
                        return Ok(Err(problem(
                            index,
                            ProblemKind::Conflict,
                            format!(
                                "{}{} in \"{}\" changed at revision {} (to {}), after revision {read_at}. Read it again and retry.",
                                column_name(*column),
                                row,
                                working_tab.name,
                                stored.revision,
                                crate::work_item_labels::echo(&stored.input)
                            ),
                        )));
                    }
                }
                operations.push((
                    index,
                    Operation::Set {
                        tab,
                        row: *row,
                        column: *column,
                        input: input.clone(),
                    },
                ));
            }
            Change::Row {
                tab,
                key_column,
                values,
            } => {
                let tab = tab!(tab);
                if values.is_empty() {
                    return Ok(Err(problem(
                        index,
                        ProblemKind::Invalid,
                        "Give the row's values by header.",
                    )));
                }
                let working_tab = &working[tab];
                if working_tab.structure_changed || working_tab.id.is_none() {
                    return Ok(Err(problem(
                        index,
                        ProblemKind::Invalid,
                        format!(
                            "\"{}\" had rows, columns or its name changed earlier in this write. Send row updates in a separate call.",
                            working_tab.name
                        ),
                    )));
                }
                let tab_cells = working_tab
                    .id
                    .and_then(|id| by_tab.get(&id))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                // Headers: row 1's values, as text, by column. Headers this
                // write already added count too.
                let mut headers: BTreeMap<i32, String> = tab_cells
                    .iter()
                    .filter(|cell| cell.row_number == 1)
                    .map(|cell| (cell.column_number, text_of(&cell.value())))
                    .filter(|(_, header)| !header.trim().is_empty())
                    .collect();
                for (_, operation) in &operations {
                    if let Operation::Set {
                        tab: set_tab,
                        row: 1,
                        column,
                        input,
                    } = operation
                        && *set_tab == tab
                    {
                        if input.is_empty() {
                            headers.remove(column);
                        } else {
                            headers.insert(*column, input.clone());
                        }
                    }
                }
                let column_of = |headers: &BTreeMap<i32, String>, name: &str| {
                    headers
                        .iter()
                        .find(|(_, header)| header.trim().eq_ignore_ascii_case(name.trim()))
                        .map(|(column, _)| *column)
                };
                let mut sets = Vec::new();
                for (header, value) in values {
                    let Some(input) = input_of(value) else {
                        return Ok(Err(problem(
                            index,
                            ProblemKind::Invalid,
                            format!(
                                "\"{}\" needs text, a number, true or false, or null to clear it.",
                                crate::work_item_labels::echo(header)
                            ),
                        )));
                    };
                    check!(check_input(index, &input));
                    let column = match column_of(&headers, header) {
                        Some(column) => column,
                        None => {
                            let Some(header_text) = normalize_header(header) else {
                                return Ok(Err(problem(
                                    index,
                                    ProblemKind::Invalid,
                                    "A header is one line of 1 to 200 characters.",
                                )));
                            };
                            let column = headers.keys().next_back().copied().unwrap_or(0) + 1;
                            check!(check_column(index, column));
                            headers.insert(column, header_text.clone());
                            operations.push((
                                index,
                                Operation::Set {
                                    tab,
                                    row: 1,
                                    column,
                                    input: header_text,
                                },
                            ));
                            column
                        }
                    };
                    sets.push((column, input));
                }
                // The row: the first under the header whose key matches.
                let key = match key_column {
                    Some(key_column) => {
                        let Some(column) = column_of(&headers, key_column) else {
                            return Ok(Err(problem(
                                index,
                                ProblemKind::NotFound,
                                format!(
                                    "No header called \"{}\" in row 1 of \"{}\".",
                                    crate::work_item_labels::echo(key_column),
                                    working_tab.name
                                ),
                            )));
                        };
                        let Some((_, key_input)) = sets.iter().find(|(set, _)| *set == column)
                        else {
                            return Ok(Err(problem(
                                index,
                                ProblemKind::Invalid,
                                format!(
                                    "Give the row's \"{}\" value: it's how the row is found.",
                                    crate::work_item_labels::echo(key_column)
                                ),
                            )));
                        };
                        Some((column, key_input.clone()))
                    }
                    None => None,
                };
                // What this write already set in the tab, over what's stored.
                let mut pending: BTreeMap<(i32, i32), &str> = BTreeMap::new();
                for (_, operation) in &operations {
                    if let Operation::Set {
                        tab: set_tab,
                        row,
                        column,
                        input,
                    } = operation
                        && *set_tab == tab
                    {
                        pending.insert((*row, *column), input.as_str());
                    }
                }
                let found = key.as_ref().and_then(|(column, key_input)| {
                    let wanted = key_input.trim().trim_start_matches('\'');
                    let stored = tab_cells
                        .iter()
                        .filter(|cell| cell.row_number > 1 && cell.column_number == *column)
                        .filter(|cell| !pending.contains_key(&(cell.row_number, *column)))
                        .filter(|cell| {
                            same_key(&text_of(&cell.value()), wanted)
                                || same_key(&cell.input, wanted)
                        })
                        .map(|cell| cell.row_number);
                    let set = pending
                        .iter()
                        .filter(|((row, set_column), input)| {
                            *row > 1 && set_column == column && same_key(input, wanted)
                        })
                        .map(|((row, _), _)| *row);
                    stored.chain(set).min()
                });
                let row = match found {
                    Some(row) => row,
                    None => {
                        let last_set = pending
                            .iter()
                            .filter(|(_, input)| !input.is_empty())
                            .map(|((row, _), _)| *row)
                            .max()
                            .unwrap_or(0);
                        let last_stored = tab_cells
                            .iter()
                            .map(|cell| cell.row_number)
                            .max()
                            .unwrap_or(0)
                            .max(last_set)
                            .max(1);
                        let next = appended
                            .get(&tab)
                            .copied()
                            .unwrap_or(last_stored)
                            .max(last_stored)
                            + 1;
                        check!(check_row(index, next));
                        appended.insert(tab, next);
                        next
                    }
                };
                for (column, input) in sets {
                    operations.push((
                        index,
                        Operation::Set {
                            tab,
                            row,
                            column,
                            input,
                        },
                    ));
                }
            }
            Change::InsertRows { tab, row, count } | Change::DeleteRows { tab, row, count } => {
                let tab = tab!(tab);
                check!(check_row(index, *row));
                check!(check_count(index, *count, MAX_ROWS));
                working[tab].structure_changed = true;
                working[tab].moves.push(Move {
                    rows: true,
                    insert: matches!(change, Change::InsertRows { .. }),
                    at: *row,
                    count: *count,
                });
                operations.push((
                    index,
                    if matches!(change, Change::InsertRows { .. }) {
                        Operation::InsertRows {
                            tab,
                            row: *row,
                            count: *count,
                        }
                    } else {
                        Operation::DeleteRows {
                            tab,
                            row: *row,
                            count: *count,
                        }
                    },
                ));
            }
            Change::InsertColumns { tab, column, count }
            | Change::DeleteColumns { tab, column, count } => {
                let tab = tab!(tab);
                check!(check_column(index, *column));
                check!(check_count(index, *count, MAX_COLUMNS));
                working[tab].structure_changed = true;
                working[tab].moves.push(Move {
                    rows: false,
                    insert: matches!(change, Change::InsertColumns { .. }),
                    at: *column,
                    count: *count,
                });
                operations.push((
                    index,
                    if matches!(change, Change::InsertColumns { .. }) {
                        Operation::InsertColumns {
                            tab,
                            column: *column,
                            count: *count,
                        }
                    } else {
                        Operation::DeleteColumns {
                            tab,
                            column: *column,
                            count: *count,
                        }
                    },
                ));
            }
            Change::AddTab { name } => {
                let Some(name) = normalize_tab_name(name) else {
                    return Ok(Err(problem(
                        index,
                        ProblemKind::Invalid,
                        "A tab's name is 1 to 31 characters, without []:*?/\\ and not starting or ending with an apostrophe.",
                    )));
                };
                if working.len() >= MAX_TABS {
                    return Ok(Err(problem(
                        index,
                        ProblemKind::Full,
                        format!("A sheet has up to {MAX_TABS} tabs."),
                    )));
                }
                if working
                    .iter()
                    .any(|tab| tab.name.eq_ignore_ascii_case(&name))
                {
                    return Ok(Err(problem(
                        index,
                        ProblemKind::Conflict,
                        format!("\"{}\" already has a tab called \"{name}\".", sheet.name),
                    )));
                }
                working.push(WorkingTab {
                    id: None,
                    name: name.clone(),
                    stored_structure_revision: 0,
                    renamed: false,
                    structure_changed: false,
                    frozen_rows: 1,
                    column_widths: serde_json::Map::new(),
                    settings_changed: false,
                    moves: Vec::new(),
                });
                operations.push((index, Operation::AddTab { name }));
            }
            Change::RenameTab { tab, name } => {
                let tab = tab!(tab);
                let Some(name) = normalize_tab_name(name) else {
                    return Ok(Err(problem(
                        index,
                        ProblemKind::Invalid,
                        "A tab's name is 1 to 31 characters, without []:*?/\\ and not starting or ending with an apostrophe.",
                    )));
                };
                if working.iter().enumerate().any(|(other, existing)| {
                    other != tab && existing.name.eq_ignore_ascii_case(&name)
                }) {
                    return Ok(Err(problem(
                        index,
                        ProblemKind::Conflict,
                        format!("\"{}\" already has a tab called \"{name}\".", sheet.name),
                    )));
                }
                if working[tab].name != name {
                    working[tab].name = name.clone();
                    working[tab].renamed = true;
                    operations.push((index, Operation::RenameTab { tab, name }));
                }
            }
            Change::RemoveTab { tab } => {
                let tab = tab!(tab);
                if working.len() <= 1 {
                    return Ok(Err(problem(
                        index,
                        ProblemKind::Invalid,
                        "A sheet keeps at least one tab. Archive the sheet instead.",
                    )));
                }
                working.remove(tab);
                appended.clear();
                operations.push((index, Operation::RemoveTab { tab }));
            }
            Change::MoveTab { tab, position } => {
                let tab = tab!(tab);
                let Some(id) = working[tab].id else {
                    return Ok(Err(problem(
                        index,
                        ProblemKind::Invalid,
                        "Move a tab this write adds in a later call.",
                    )));
                };
                if *position < 1 || *position as usize > working.len() {
                    return Ok(Err(problem(
                        index,
                        ProblemKind::Invalid,
                        format!("position goes from 1 to {}.", working.len()),
                    )));
                }
                moves.push((index, id, *position));
            }
            Change::FreezeRows { tab, rows } => {
                let tab = tab!(tab);
                if !(0..=MAX_FROZEN_ROWS).contains(rows) {
                    return Ok(Err(problem(
                        index,
                        ProblemKind::Invalid,
                        format!("Freeze 0 to {MAX_FROZEN_ROWS} rows."),
                    )));
                }
                if working[tab].frozen_rows != *rows {
                    working[tab].frozen_rows = *rows;
                    working[tab].settings_changed = true;
                }
            }
            Change::ColumnWidth { tab, column, width } => {
                let tab = tab!(tab);
                check!(check_column(index, *column));
                let key = column.to_string();
                match width {
                    Some(width) if (MIN_COLUMN_WIDTH..=MAX_COLUMN_WIDTH).contains(width) => {
                        if working[tab].column_widths.get(&key) != Some(&json!(width)) {
                            working[tab].column_widths.insert(key, json!(width));
                            working[tab].settings_changed = true;
                        }
                    }
                    Some(_) => {
                        return Ok(Err(problem(
                            index,
                            ProblemKind::Invalid,
                            format!(
                                "A column is {MIN_COLUMN_WIDTH} to {MAX_COLUMN_WIDTH} pixels wide; null puts it back."
                            ),
                        )));
                    }
                    None => {
                        if working[tab].column_widths.remove(&key).is_some() {
                            working[tab].settings_changed = true;
                        }
                    }
                }
            }
        }
    }

    // The engine works through the cell and tab operations.
    let inputs = stored_tabs
        .iter()
        .map(|tab| TabInput {
            name: tab.name.clone(),
            cells: by_tab
                .get(&tab.id)
                .map(|cells| {
                    cells
                        .iter()
                        .map(|cell| (cell.row_number, cell.column_number, cell.input.clone()))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    let (indexes, engine_operations): (Vec<usize>, Vec<Operation>) = operations.into_iter().unzip();
    let outputs = if engine_operations.is_empty() {
        None
    } else {
        let engine = engine.clone();
        let result = tokio::task::spawn_blocking(move || engine.apply(&inputs, &engine_operations))
            .await
            .map_err(|_| AppError::Internal("sheet engine task"))?;
        match result {
            Ok(outputs) => Some(outputs),
            Err(error) => {
                let index = error
                    .operation
                    .and_then(|operation| indexes.get(operation).copied())
                    .unwrap_or(0);
                return Ok(Err(problem(
                    index,
                    ProblemKind::Invalid,
                    format!("The sheet couldn't make that change: {}", error.message),
                )));
            }
        }
    };

    let revision = sheet.revision + 1;
    let mut inputs_changed = 0;
    let mut values_changed = 0;
    let mut changed = Vec::new();
    let mut tabs_changed = false;

    // New tabs get IDs; every tab gets its position.
    let mut ids = Vec::with_capacity(working.len());
    for tab in &working {
        ids.push(tab.id.unwrap_or_else(Uuid::now_v7));
    }
    if let Some(outputs) = &outputs {
        if outputs.len() != working.len() {
            return Err(AppError::Internal("sheet engine tab count"));
        }
        let total = outputs.iter().map(|tab| tab.cells.len()).sum::<usize>();
        if total > MAX_CELLS {
            return Ok(Err(problem(
                0,
                ProblemKind::Full,
                format!("A sheet holds up to {MAX_CELLS} filled cells; this would make {total}."),
            )));
        }
        for (tab, output) in outputs.iter().enumerate() {
            if let Some(cell) = output
                .cells
                .iter()
                .find(|cell| cell.row > MAX_ROWS || cell.column > MAX_COLUMNS)
            {
                return Ok(Err(problem(
                    0,
                    ProblemKind::Full,
                    format!(
                        "That would push {}{} of \"{}\" past the sheet's edge ({}{MAX_ROWS}).",
                        column_name(cell.column),
                        cell.row,
                        working[tab].name,
                        column_name(MAX_COLUMNS)
                    ),
                )));
            }
        }
    }

    // Tabs: removed ones go (with their cells), added ones come, the rest
    // take their names, positions and settings.
    let kept: Vec<Uuid> = working.iter().filter_map(|tab| tab.id).collect();
    let removed = stored_tabs
        .iter()
        .filter(|tab| !kept.contains(&tab.id))
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    if !removed.is_empty() {
        tabs_changed = true;
        sqlx::query("DELETE FROM sheet_cells WHERE sheet_id = $1 AND tab_id = ANY($2)")
            .bind(sheet.id)
            .bind(&removed)
            .execute(&mut *connection)
            .await?;
        sqlx::query("DELETE FROM sheet_tabs WHERE sheet_id = $1 AND id = ANY($2)")
            .bind(sheet.id)
            .bind(&removed)
            .execute(&mut *connection)
            .await?;
    }
    // Renamed tabs take a stand-in name first, so renames that swap or
    // chain names (B to C, then A to B) never meet a name not yet freed.
    for tab in working.iter().filter(|tab| tab.renamed) {
        if let Some(id) = tab.id {
            let simple = id.simple().to_string();
            sqlx::query("UPDATE sheet_tabs SET name = $3 WHERE sheet_id = $1 AND id = $2")
                .bind(sheet.id)
                .bind(id)
                .bind(format!("~{}", &simple[simple.len() - 24..]))
                .execute(&mut *connection)
                .await?;
        }
    }
    // Final order: working order, then the moves in turn.
    let mut order = ids.clone();
    for (_, id, position) in &moves {
        if let Some(from) = order.iter().position(|existing| existing == id) {
            let id = order.remove(from);
            order.insert((*position as usize - 1).min(order.len()), id);
        }
    }
    // Positions are unique only by convention; park them first so a swap
    // never trips over itself.
    let stored_positions: HashMap<Uuid, i32> = stored_tabs
        .iter()
        .map(|tab| (tab.id, tab.position))
        .collect();
    for (tab, id) in working.iter().zip(&ids) {
        let position = order
            .iter()
            .position(|existing| existing == id)
            .unwrap_or(0) as i32;
        match tab.id {
            None => {
                tabs_changed = true;
                sqlx::query(
                    "INSERT INTO sheet_tabs \
                       (id, project_id, sheet_id, name, position, structure_revision, frozen_rows, \
                        column_widths) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                )
                .bind(id)
                .bind(sheet.project_id)
                .bind(sheet.id)
                .bind(&tab.name)
                .bind(position)
                .bind(revision)
                .bind(tab.frozen_rows)
                .bind(Json::Object(tab.column_widths.clone()))
                .execute(&mut *connection)
                .await?;
            }
            Some(_) => {
                let moved = stored_positions.get(id) != Some(&position);
                if tab.renamed || tab.structure_changed || tab.settings_changed || moved {
                    tabs_changed = true;
                    sqlx::query(
                        "UPDATE sheet_tabs SET name = $3, position = $4, \
                           structure_revision = CASE WHEN $5 THEN $6 ELSE structure_revision END, \
                           frozen_rows = $7, column_widths = $8, updated_at = now() \
                         WHERE sheet_id = $1 AND id = $2",
                    )
                    .bind(sheet.id)
                    .bind(id)
                    .bind(&tab.name)
                    .bind(position)
                    .bind(tab.renamed || tab.structure_changed)
                    .bind(revision)
                    .bind(tab.frozen_rows)
                    .bind(Json::Object(tab.column_widths.clone()))
                    .execute(&mut *connection)
                    .await?;
                }
            }
        }
    }

    // Cells: what the engine says now against what's stored.
    if let Some(outputs) = &outputs {
        let mut upsert_tabs = Vec::new();
        let mut upsert_rows = Vec::new();
        let mut upsert_columns = Vec::new();
        let mut upsert_inputs = Vec::new();
        let mut upsert_kinds = Vec::new();
        let mut upsert_values = Vec::new();
        let mut upsert_revisions = Vec::new();
        let mut seen: std::collections::HashSet<(Uuid, i32, i32)> =
            std::collections::HashSet::new();
        let mut input_cells = Vec::new();
        let mut value_cells = Vec::new();
        for (tab, output) in outputs.iter().enumerate() {
            let id = ids[tab];
            for cell in &output.cells {
                seen.insert((id, cell.row, cell.column));
                let stored = stored_at.get(&(id, cell.row, cell.column));
                let (input_changed, value_changed) = match stored {
                    Some(stored) => (
                        stored.input != cell.input,
                        stored.value() != cell.value || stored.kind != cell.value.kind(),
                    ),
                    None => (true, true),
                };
                if !input_changed && !value_changed {
                    continue;
                }
                upsert_tabs.push(id);
                upsert_rows.push(cell.row);
                upsert_columns.push(cell.column);
                upsert_inputs.push(cell.input.clone());
                upsert_kinds.push(cell.value.kind().to_owned());
                upsert_values.push(cell.value.to_json().to_string());
                upsert_revisions.push(if input_changed {
                    revision
                } else {
                    stored.map(|stored| stored.revision).unwrap_or(revision)
                });
                let line = ChangedCell {
                    tab: output.name.clone(),
                    row: cell.row,
                    column: cell.column,
                    input: Some(cell.input.clone()),
                    value: cell.value.clone(),
                };
                if input_changed {
                    inputs_changed += 1;
                    input_cells.push(line);
                } else {
                    values_changed += 1;
                    value_cells.push(line);
                }
            }
        }
        // Cells that are gone (cleared, or moved off by a delete). Cells
        // of removed tabs went with them.
        let mut delete_tabs = Vec::new();
        let mut delete_rows = Vec::new();
        let mut delete_columns = Vec::new();
        for cell in &stored_cells {
            if removed.contains(&cell.tab_id) {
                continue;
            }
            if !seen.contains(&(cell.tab_id, cell.row_number, cell.column_number)) {
                delete_tabs.push(cell.tab_id);
                delete_rows.push(cell.row_number);
                delete_columns.push(cell.column_number);
                inputs_changed += 1;
                let tab_name = working
                    .iter()
                    .zip(&ids)
                    .find(|(_, id)| **id == cell.tab_id)
                    .map(|(tab, _)| tab.name.clone())
                    .unwrap_or_default();
                input_cells.push(ChangedCell {
                    tab: tab_name,
                    row: cell.row_number,
                    column: cell.column_number,
                    input: None,
                    value: Value::Empty,
                });
            }
        }
        if !delete_tabs.is_empty() {
            sqlx::query(
                "DELETE FROM sheet_cells cell \
                 USING UNNEST($2::uuid[], $3::int[], $4::int[]) AS gone(tab_id, row_number, column_number) \
                 WHERE cell.sheet_id = $1 AND cell.tab_id = gone.tab_id \
                   AND cell.row_number = gone.row_number AND cell.column_number = gone.column_number",
            )
            .bind(sheet.id)
            .bind(&delete_tabs)
            .bind(&delete_rows)
            .bind(&delete_columns)
            .execute(&mut *connection)
            .await?;
        }
        if !upsert_tabs.is_empty() {
            sqlx::query(
                "INSERT INTO sheet_cells \
                   (project_id, sheet_id, tab_id, row_number, column_number, input, kind, value, \
                    revision, updated_by_actor_id) \
                 SELECT $1, $2, cell.tab_id, cell.row_number, cell.column_number, cell.input, \
                        cell.kind, cell.value::jsonb, cell.revision, $3 \
                 FROM UNNEST($4::uuid[], $5::int[], $6::int[], $7::text[], $8::text[], \
                             $9::text[], $10::bigint[]) \
                      AS cell(tab_id, row_number, column_number, input, kind, value, revision) \
                 ON CONFLICT (tab_id, row_number, column_number) DO UPDATE SET \
                   input = EXCLUDED.input, kind = EXCLUDED.kind, value = EXCLUDED.value, \
                   revision = EXCLUDED.revision, \
                   updated_by_actor_id = CASE WHEN sheet_cells.input IS DISTINCT FROM EXCLUDED.input \
                     THEN EXCLUDED.updated_by_actor_id ELSE sheet_cells.updated_by_actor_id END, \
                   updated_at = CASE WHEN sheet_cells.input IS DISTINCT FROM EXCLUDED.input \
                     THEN now() ELSE sheet_cells.updated_at END",
            )
            .bind(sheet.project_id)
            .bind(sheet.id)
            .bind(actor_id)
            .bind(&upsert_tabs)
            .bind(&upsert_rows)
            .bind(&upsert_columns)
            .bind(&upsert_inputs)
            .bind(&upsert_kinds)
            .bind(&upsert_values)
            .bind(&upsert_revisions)
            .execute(&mut *connection)
            .await?;
        }
        // Clears are remembered, so an edit read before one is refused like
        // any other. A tab whose rows or columns moved forgets them: its new
        // structure revision refuses older edits anyway.
        let moved_tabs = working
            .iter()
            .zip(&ids)
            .filter(|(tab, _)| tab.id.is_some() && tab.structure_changed)
            .map(|(_, id)| *id)
            .collect::<Vec<_>>();
        if !moved_tabs.is_empty() {
            sqlx::query("DELETE FROM sheet_cleared_cells WHERE tab_id = ANY($1)")
                .bind(&moved_tabs)
                .execute(&mut *connection)
                .await?;
        }
        let mut clear_tabs = Vec::new();
        let mut clear_rows = Vec::new();
        let mut clear_columns = Vec::new();
        for ((tab, row), column) in delete_tabs.iter().zip(&delete_rows).zip(&delete_columns) {
            if !moved_tabs.contains(tab) {
                clear_tabs.push(*tab);
                clear_rows.push(*row);
                clear_columns.push(*column);
            }
        }
        if !clear_tabs.is_empty() {
            sqlx::query(
                "INSERT INTO sheet_cleared_cells (tab_id, row_number, column_number, revision) \
                 SELECT gone.tab_id, gone.row_number, gone.column_number, $4 \
                 FROM UNNEST($1::uuid[], $2::int[], $3::int[]) AS gone(tab_id, row_number, column_number) \
                 ON CONFLICT (tab_id, row_number, column_number) DO UPDATE SET revision = EXCLUDED.revision",
            )
            .bind(&clear_tabs)
            .bind(&clear_rows)
            .bind(&clear_columns)
            .bind(revision)
            .execute(&mut *connection)
            .await?;
        }
        if !upsert_tabs.is_empty() {
            sqlx::query(
                "DELETE FROM sheet_cleared_cells cleared \
                 USING UNNEST($1::uuid[], $2::int[], $3::int[]) AS filled(tab_id, row_number, column_number) \
                 WHERE cleared.tab_id = filled.tab_id AND cleared.row_number = filled.row_number \
                   AND cleared.column_number = filled.column_number",
            )
            .bind(&upsert_tabs)
            .bind(&upsert_rows)
            .bind(&upsert_columns)
            .execute(&mut *connection)
            .await?;
        }
        changed = input_cells;
        changed.extend(value_cells);
        changed.truncate(limit);
    }

    if inputs_changed == 0 && values_changed == 0 && !tabs_changed {
        return Ok(Ok(Applied {
            revision: sheet.revision,
            inputs_changed,
            values_changed,
            tabs_changed,
            changed,
        }));
    }
    // A value can change with no input changing (NOW(), RAND()); that isn't
    // a new revision unless something else changed.
    if inputs_changed == 0 && !tabs_changed {
        return Ok(Ok(Applied {
            revision: sheet.revision,
            inputs_changed,
            values_changed,
            tabs_changed,
            changed,
        }));
    }
    sqlx::query(
        "UPDATE sheets SET revision = $3, updated_by_actor_id = $4, updated_at = now() \
         WHERE project_id = $1 AND id = $2",
    )
    .bind(sheet.project_id)
    .bind(sheet.id)
    .bind(revision)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    let mut kinds = changes.iter().map(Change::short).collect::<Vec<_>>();
    kinds.dedup();
    kinds.truncate(50);
    insert_revision(
        &mut *connection,
        sheet.project_id,
        sheet.id,
        revision,
        actor_id,
        json!({
            "cells": inputs_changed,
            "values": values_changed,
            "tabs": tabs_changed,
            "operations": kinds,
        }),
    )
    .await?;
    Ok(Ok(Applied {
        revision,
        inputs_changed,
        values_changed,
        tabs_changed,
        changed,
    }))
}

/// A header as row 1 holds it: one line of up to 200 characters.
fn normalize_header(value: &str) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    let value = collapse(value);
    let length = value.chars().count();
    (length > 0 && length <= 200 && !value.starts_with('=')).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_spreadsheet_rules() {
        assert_eq!(
            normalize_tab_name("  Q3   plan "),
            Some("Q3 plan".to_owned())
        );
        assert_eq!(normalize_tab_name("a/b"), None);
        assert_eq!(normalize_tab_name("'quoted"), None);
        assert_eq!(normalize_tab_name(&"x".repeat(32)), None);
        assert_eq!(
            normalize_name("Budget 2027"),
            Some("Budget 2027".to_owned())
        );
        assert_eq!(normalize_name(&Uuid::now_v7().to_string()), None);
        assert_eq!(normalize_header("=SUM(A1)"), None);
    }

    #[test]
    fn values_read_as_text_for_keys() {
        assert_eq!(text_of(&Value::Number(12.0)), "12");
        assert_eq!(text_of(&Value::Number(1.5)), "1.5");
        assert_eq!(text_of(&Value::Boolean(true)), "TRUE");
        assert_eq!(input_of(&json!(3)), Some("3".to_owned()));
        assert_eq!(input_of(&json!(null)), Some(String::new()));
        assert_eq!(input_of(&json!([1])), None);
    }
}

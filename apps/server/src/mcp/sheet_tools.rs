//! Sheet tools (AWD-54): three tools for every sheet a project has.
//!
//! - `sheet_list`: a project's sheets, with their tabs and revision.
//! - `sheet_read`: a range of one tab's cells (values, and formulas where
//!   there are any), or its rows as objects by header, filtered.
//! - `sheet_write`: create a sheet, rename, describe, archive or restore
//!   it, and change its cells, rows, tabs and formats, all or nothing
//!   (`sheets.edit`).
//!
//! The rules are `crate::sheets`, the same the web app will use. A sheet is
//! looked up only among the project's own (`principal.project_id`), and tabs
//! only among the sheet's, so an ID from anywhere else never matches.

use std::collections::BTreeMap;

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ARCHIVED_PROJECT_MESSAGE, StoredRole, ToolResult, insert_audit, link_url, mark_via,
        parse_id, role_allows,
    },
    talk::can_write_in,
    writes::{REQUEST_ID_MESSAGE, parse_request_id},
};
use crate::{
    AppState,
    error::AppError,
    idempotency::{IdempotencyKey, IdempotencyOutcome, RequestHash, reserve, resource_bound_scope},
    memory,
    policy::Action,
    sheets::{
        self, Applied, Change, ChangeOutcome, CreateOutcome, ProblemKind, Sheet, SheetChange,
        SheetSummary, StoredCell, Tab,
        address::{self, column_name},
        engine::Value as CellValue,
    },
    work_item_commands::{lock_membership, share_membership},
    work_item_labels::echo,
};

const MESSAGES_SPACE_MESSAGE: &str =
    "The Messages space has no sheets. Pick a project with projects_list.";
const EDIT_MESSAGE: &str =
    "Your role here can't edit sheets (sheets.edit). permissions_get says what it can do.";
/// Cells one read returns at most.
const MAX_READ_CELLS: i64 = 2_000;
/// Rows one read returns at most, as objects.
const MAX_READ_ROWS: usize = 500;
/// Changed cells a write returns at most.
const MAX_ECHOED_CELLS: usize = 200;

fn sheet_url(state: &AppState, project_id: Uuid, sheet_id: Uuid) -> String {
    link_url(
        state,
        None,
        &crate::addresses::Target::space(project_id, crate::addresses::Place::Sheet(sheet_id)),
    )
}

/// AWD-75: whether Sheets is off in the project, for its workspace or the
/// caller's role there.
async fn module_off(state: &AppState, principal: &McpPrincipal) -> Result<bool, AppError> {
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let projects = [principal.project_id];
    Ok(
        crate::modules::state::ProjectModules::load(&mut connection, principal.actor_id, &projects)
            .await?
            .with(&projects, "sheets")
            .is_empty(),
    )
}

/// Whether the caller may read the project's sheets: anyone in it. `Err`
/// carries the refusal.
async fn read_access<T>(
    state: &AppState,
    principal: &McpPrincipal,
) -> Result<Result<(), ToolResult<T>>, AppError> {
    match memory::project_access(
        &state.pool,
        principal.actor_id,
        principal.project_id,
        Action::ProjectView,
    )
    .await
    {
        Ok(false) => Ok(Ok(())),
        Ok(true) => Ok(Err(ToolResult::invalid(
            "projectId",
            MESSAGES_SPACE_MESSAGE,
        ))),
        Err(AppError::NotFound(_)) => Ok(Err(ToolResult::not_found("That project"))),
        Err(error) => Err(error),
    }
}

/// A write holding the project lock (or, for changes to one sheet's cells
/// and tabs, only the membership; C1.12), with the role checked again under
/// it. `suggest_ok` lets a role without `sheets.edit` through for a change
/// to an existing sheet's cells, which the caller then checks with
/// `access::write_mode` once it knows the sheet (C1.11); the flag returned
/// says whether the role edits sheets.
async fn begin_write<T>(
    state: &AppState,
    principal: &McpPrincipal,
    project_lock: bool,
    suggest_ok: bool,
) -> Result<Result<(Transaction<'static, Postgres>, bool), ToolResult<T>>, AppError> {
    let project_id = principal.project_id;
    let edits = can_write_in(state, principal, project_id, Action::SheetEdit).await?;
    if !edits && !suggest_ok {
        return Ok(Err(ToolResult::denied(EDIT_MESSAGE)));
    }
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    mark_via(&mut transaction, principal).await?;
    let membership = if project_lock {
        lock_membership(&mut transaction, principal.actor_id, project_id).await?
    } else {
        share_membership(&mut transaction, principal.actor_id, project_id).await?
    };
    let Some(membership) = membership else {
        return Ok(Err(ToolResult::not_found("That project")));
    };
    if !membership.enabled {
        return Ok(Err(ToolResult::not_found("That project")));
    }
    if membership.project_archived_at.is_some() {
        return Ok(Err(ToolResult::conflict(ARCHIVED_PROJECT_MESSAGE)));
    }
    let edits = edits
        && role_allows(
            principal,
            project_id,
            &StoredRole::of(&membership.role, membership.role_permissions.as_deref())
                .with_groups(membership.group_permissions.clone()),
            Action::SheetEdit,
        );
    if !edits && !suggest_ok {
        return Ok(Err(ToolResult::denied(EDIT_MESSAGE)));
    }
    let kind = sqlx::query_scalar::<_, String>("SELECT kind FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_one(&mut *transaction)
        .await?;
    if kind == "direct" {
        return Ok(Err(ToolResult::invalid(
            "projectId",
            MESSAGES_SPACE_MESSAGE,
        )));
    }
    Ok(Ok((transaction, edits)))
}

fn request_key(value: Option<&str>) -> Option<IdempotencyKey> {
    match value {
        Some(value) => parse_request_id(value),
        None => parse_request_id(&Uuid::now_v7().to_string()),
    }
}

async fn find_sheet<T>(
    connection: &mut PgConnection,
    project_id: Uuid,
    reference: &str,
) -> Result<Result<Sheet, ToolResult<T>>, AppError> {
    match sheets::find(&mut *connection, project_id, reference).await? {
        Some(sheet) => Ok(Ok(sheet)),
        None => Ok(Err(ToolResult::NotFound {
            message: format!(
                "No sheet called \"{}\" here. Its sheets: {}. sheet_list shows them.",
                echo(reference),
                sheets::active_names(&mut *connection, project_id).await?
            ),
        })),
    }
}

/// A value as JSON: numbers, text, true or false, null for empty; an
/// error is its code, such as "#DIV/0!".
fn json_of(value: &CellValue) -> Value {
    value.to_json()
}

// ---------------------------------------------------------------------------
// sheet_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SheetListArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// Also list archived sheets, which sheet_write can restore.
    pub include_archived: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SheetLine {
    #[serde(flatten)]
    pub sheet: SheetSummary,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SheetListValue {
    /// The project's sheets, last changed first.
    pub sheets: Vec<SheetLine>,
    /// Whether you can create sheets and change them.
    pub can_edit: bool,
}

pub(super) async fn sheet_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SheetListArguments,
) -> ToolResult<SheetListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let result = async {
        if let Err(refusal) = read_access(state, principal).await? {
            return Ok(refusal);
        }
        if module_off(state, principal).await? {
            return Ok(ToolResult::module_off("sheet_list", "Sheets"));
        }
        let listed = sheets::list(
            &state.pool,
            principal.project_id,
            arguments.include_archived.unwrap_or(false),
        )
        .await?;
        let project_open =
            sqlx::query_scalar::<_, bool>("SELECT archived_at IS NULL FROM projects WHERE id = $1")
                .bind(principal.project_id)
                .fetch_one(crate::db::scoped(&state.pool))
                .await?;
        let can_edit = project_open
            && can_write_in(state, principal, principal.project_id, Action::SheetEdit).await?;
        Ok::<_, AppError>(ToolResult::ok(SheetListValue {
            sheets: listed
                .into_iter()
                .map(|sheet| SheetLine {
                    url: sheet
                        .id
                        .parse::<Uuid>()
                        .map(|id| sheet_url(state, principal.project_id, id))
                        .unwrap_or_default(),
                    sheet,
                })
                .collect(),
            can_edit,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "sheet_list"))
}

// ---------------------------------------------------------------------------
// sheet_read

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SheetReadArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The sheet, by name or ID.
    pub sheet: String,
    /// The tab, by name or ID. Defaults to the first.
    pub tab: Option<String>,
    /// Cells to read in A1 form: "A1:D20", "B:D" (whole columns), "2:10"
    /// (whole rows), "C4" or "Tab!A1:D20", or a named range's name (which
    /// picks its tab). Defaults to everything the tab uses. At most 2,000
    /// filled cells come back.
    pub range: Option<String>,
    /// Read the tab as rows instead of cells: row 1 is the header, and each
    /// row below comes back as {header: value} with its row number.
    pub as_rows: Option<bool>,
    /// With asRows: keep only rows whose value under each header equals the
    /// one given (text compares ignoring case; numbers and true/false as
    /// they are).
    #[schemars(with = "Option<Map<String, Value>>")]
    #[serde(rename = "where")]
    pub filter: Option<Map<String, Value>>,
    /// With asRows: rows to skip first (default 0).
    pub offset: Option<usize>,
    /// With asRows: rows to return (default 100, at most 500).
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TabLine {
    pub id: String,
    pub name: String,
    /// The last row and column holding something, such as "F120"; empty
    /// when the tab is.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub used: String,
    /// Rows at the top that stay put while the rest scrolls.
    pub frozen_rows: i32,
    /// Its conditional formats, in order (the first that applies wins).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditional_formats: Vec<sheets::conditional::ConditionalRule>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct CellLine {
    /// Such as "B12".
    pub cell: String,
    /// What it computes to: a number, text, true or false, or an error
    /// code such as "#DIV/0!".
    #[schemars(with = "Value")]
    pub value: Value,
    /// The formula, for cells that have one (starts with "=").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// "error" when the value is an error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The value as the cell shows it, for a cell with a number format
    /// (such as "10%" for 0.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// Its number format, such as "0.0%", when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number_format: Option<String>,
    /// Its style (bold, italic, underline, strike, color, fill, align,
    /// wrap), when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<sheets::CellStyle>,
    /// How it looks once the tab's conditional formats apply, when that
    /// differs from its own style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditional_style: Option<sheets::CellStyle>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RowLine {
    /// The row's number in the tab.
    pub row: i32,
    /// Its values by header.
    #[schemars(with = "Map<String, Value>")]
    pub values: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SheetReadValue {
    pub sheet: SheetSummary,
    /// Every tab, in order.
    pub tabs: Vec<TabLine>,
    /// The tab read.
    pub tab: String,
    /// The block read, such as "A1:F120".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub range: String,
    /// The filled cells in the range, row by row (empty cells are left
    /// out).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<CellLine>,
    /// With asRows: row 1's headers, in column order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<String>,
    /// With asRows: the rows, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<RowLine>,
    /// With asRows: rows that matched, before offset and limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matching_rows: Option<usize>,
    /// Whether more cells or rows matched than came back: narrow the range
    /// or page with offset.
    pub truncated: bool,
    pub can_edit: bool,
    pub url: String,
    /// The sheet's named ranges: pass a name as range to read one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub named_ranges: Vec<sheets::ranges::NamedRange>,
}

fn tab_line(tab: &Tab, used: (i32, i32)) -> TabLine {
    TabLine {
        id: tab.id.to_string(),
        name: tab.name.clone(),
        used: if used.0 == 0 {
            String::new()
        } else {
            format!("{}{}", column_name(used.1), used.0)
        },
        frozen_rows: tab.frozen_rows,
        conditional_formats: tab.rules(),
    }
}

fn cell_line(cell: &StoredCell) -> CellLine {
    let value = cell.value();
    let look = cell.look();
    CellLine {
        cell: format!("{}{}", column_name(cell.column_number), cell.row_number),
        formula: cell.input.starts_with('=').then(|| cell.input.clone()),
        kind: matches!(value, CellValue::Error(_)).then(|| "error".to_owned()),
        value: json_of(&value),
        display: cell.display.clone(),
        number_format: look.format,
        style: (!look.style.is_plain()).then_some(look.style),
        conditional_style: cell.conditional_style(),
    }
}

fn matches_filter(wanted: &Value, value: &CellValue) -> bool {
    match (wanted, value) {
        (Value::Null, CellValue::Empty) => true,
        (Value::String(text), _) => sheets::text_of(value)
            .trim()
            .eq_ignore_ascii_case(text.trim()),
        (Value::Number(number), CellValue::Number(have)) => number
            .as_f64()
            .is_some_and(|number| (number - have).abs() < 1e-9),
        (Value::Bool(wanted), CellValue::Boolean(have)) => wanted == have,
        _ => false,
    }
}

pub(super) async fn sheet_read(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SheetReadArguments,
) -> ToolResult<SheetReadValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let result = async {
        if let Err(refusal) = read_access(state, principal).await? {
            return Ok(refusal);
        }
        if module_off(state, principal).await? {
            return Ok(ToolResult::module_off("sheet_read", "Sheets"));
        }
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        let sheet =
            match find_sheet(&mut connection, principal.project_id, &arguments.sheet).await? {
                Ok(sheet) => sheet,
                Err(refusal) => return Ok(refusal),
            };
        // A named range, or a block naming its tab ("Summary!A1:B9"),
        // picks the tab itself (C1.20).
        let pointed = match arguments.range.as_deref() {
            Some(text) if sheets::ranges::name_of(text).is_some() || text.contains('!') => {
                sheets::ranges::resolve(&mut connection, sheet.id, text).await?
            }
            _ => None,
        };
        let named_ranges = sheets::ranges::named(&mut *connection, sheet.id).await?;
        let tabs = sheets::tabs(&mut *connection, sheet.id).await?;
        let pointed_tab = pointed.as_ref().map(|found| found.tab_id.to_string());
        let wanted = pointed_tab
            .as_deref()
            .or(arguments.tab.as_deref())
            .map(str::trim)
            .unwrap_or_default();
        let wanted_id = wanted.parse::<Uuid>().ok();
        let Some(tab) = (if wanted.is_empty() {
            tabs.first()
        } else {
            tabs.iter()
                .find(|tab| Some(tab.id) == wanted_id || tab.name.eq_ignore_ascii_case(wanted))
        }) else {
            return Ok(ToolResult::NotFound {
                message: format!(
                    "No tab called \"{}\" in \"{}\". Its tabs: {}.",
                    echo(wanted),
                    sheet.name,
                    tabs.iter()
                        .map(|tab| tab.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        };
        let mut tab_lines = Vec::with_capacity(tabs.len());
        let mut used = (0, 0);
        for each in &tabs {
            let extent = sheets::used_extent(&mut *connection, each.id).await?;
            if each.id == tab.id {
                used = extent;
            }
            tab_lines.push(tab_line(each, extent));
        }
        let cells_total = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sheet_cells WHERE sheet_id = $1 AND input <> ''",
        )
        .bind(sheet.id)
        .fetch_one(&mut *connection)
        .await?;
        let summary = SheetSummary {
            id: sheet.id.to_string(),
            name: sheet.name.clone(),
            description: sheet.description.clone(),
            tabs: tabs.iter().map(|tab| tab.name.clone()).collect(),
            cells: cells_total,
            revision: sheet.revision,
            archived: sheet.archived,
            created_at: sheet.created_at.clone(),
            updated_at: sheet.updated_at.clone(),
        };
        let project_open =
            sqlx::query_scalar::<_, bool>("SELECT archived_at IS NULL FROM projects WHERE id = $1")
                .bind(principal.project_id)
                .fetch_one(&mut *connection)
                .await?;
        let can_edit = project_open
            && !sheet.archived
            && can_write_in(state, principal, principal.project_id, Action::SheetEdit).await?;
        let url = sheet_url(state, principal.project_id, sheet.id);

        if arguments.as_rows.unwrap_or(false) {
            if used.0 == 0 {
                return Ok(ToolResult::ok(SheetReadValue {
                    sheet: summary,
                    tabs: tab_lines,
                    tab: tab.name.clone(),
                    range: String::new(),
                    cells: Vec::new(),
                    headers: Vec::new(),
                    rows: Vec::new(),
                    matching_rows: Some(0),
                    truncated: false,
                    can_edit,
                    url,
                    named_ranges,
                }));
            }
            let all = sheets::cells_in(
                &mut *connection,
                tab.id,
                (1, used.0),
                (1, used.1),
                i64::try_from(sheets::MAX_CELLS).unwrap_or(i64::MAX),
                true,
            )
            .await?;
            let headers: BTreeMap<i32, String> = all
                .iter()
                .filter(|cell| cell.row_number == 1)
                .map(|cell| (cell.column_number, sheets::text_of(&cell.value())))
                .filter(|(_, header)| !header.trim().is_empty())
                .collect();
            if let Some(filter) = &arguments.filter {
                for name in filter.keys() {
                    if !headers
                        .values()
                        .any(|header| header.trim().eq_ignore_ascii_case(name.trim()))
                    {
                        return Ok(ToolResult::invalid(
                            "where",
                            format!(
                                "No header called \"{}\" in row 1 of \"{}\". Its headers: {}.",
                                echo(name),
                                tab.name,
                                headers.values().cloned().collect::<Vec<_>>().join(", ")
                            ),
                        ));
                    }
                }
            }
            let mut rows: BTreeMap<i32, BTreeMap<i32, CellValue>> = BTreeMap::new();
            for cell in all.iter().filter(|cell| cell.row_number > 1) {
                rows.entry(cell.row_number)
                    .or_default()
                    .insert(cell.column_number, cell.value());
            }
            let matching = rows
                .into_iter()
                .filter(|(_, values)| {
                    arguments.filter.as_ref().is_none_or(|filter| {
                        filter.iter().all(|(name, wanted)| {
                            let column = headers
                                .iter()
                                .find(|(_, header)| header.trim().eq_ignore_ascii_case(name.trim()))
                                .map(|(column, _)| *column);
                            let value = column
                                .and_then(|column| values.get(&column))
                                .cloned()
                                .unwrap_or(CellValue::Empty);
                            matches_filter(wanted, &value)
                        })
                    })
                })
                .collect::<Vec<_>>();
            let offset = arguments.offset.unwrap_or(0);
            let limit = arguments.limit.unwrap_or(100).clamp(1, MAX_READ_ROWS);
            let matching_rows = matching.len();
            let truncated = matching_rows > offset + limit;
            let rows = matching
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|(row, values)| RowLine {
                    row,
                    values: headers
                        .iter()
                        .map(|(column, header)| {
                            (
                                header.clone(),
                                values.get(column).map(json_of).unwrap_or(Value::Null),
                            )
                        })
                        .collect(),
                })
                .collect();
            return Ok(ToolResult::ok(SheetReadValue {
                sheet: summary,
                tabs: tab_lines,
                tab: tab.name.clone(),
                range: format!("A1:{}{}", column_name(used.1), used.0),
                cells: Vec::new(),
                headers: headers.into_values().collect(),
                rows,
                matching_rows: Some(matching_rows),
                truncated,
                can_edit,
                url,
                named_ranges,
            }));
        }

        let range = match (pointed.as_ref(), arguments.range.as_deref()) {
            (Some(found), _) => found.range,
            (None, Some(text)) => match address::range(text, sheets::MAX_ROWS, sheets::MAX_COLUMNS) {
                Some(range) => range,
                None => {
                    return Ok(ToolResult::invalid(
                        "range",
                        "Name cells in A1 form (\"A1:D20\", \"B:D\", \"2:10\", \"C4\", \"Tab!A1:D20\") or by a named range of the sheet.",
                    ));
                }
            },
            (None, None) => address::Range {
                rows: (1, used.0.max(1)),
                columns: (1, used.1.max(1)),
            },
        };
        // A whole column or row reads only as far as the tab goes.
        let range = address::Range {
            rows: (range.rows.0, range.rows.1.min(used.0.max(range.rows.0))),
            columns: (
                range.columns.0,
                range.columns.1.min(used.1.max(range.columns.0)),
            ),
        };
        let found = sheets::cells_in(
            &mut *connection,
            tab.id,
            range.rows,
            range.columns,
            MAX_READ_CELLS + 1,
            true,
        )
        .await?;
        let truncated = found.len() as i64 > MAX_READ_CELLS;
        Ok::<_, AppError>(ToolResult::ok(SheetReadValue {
            sheet: summary,
            tabs: tab_lines,
            tab: tab.name.clone(),
            range: range.name(),
            cells: found
                .iter()
                .take(MAX_READ_CELLS as usize)
                .map(cell_line)
                .collect(),
            headers: Vec::new(),
            rows: Vec::new(),
            matching_rows: None,
            truncated,
            can_edit,
            url,
            named_ranges,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "sheet_read"))
}

// ---------------------------------------------------------------------------
// sheet_write

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CellWrite {
    /// The tab, by name or ID. Defaults to the first.
    pub tab: Option<String>,
    /// The cell, such as "B12".
    pub cell: String,
    /// What to put in it, as you'd type it: text, a number, TRUE or FALSE,
    /// or a formula starting with "=" (such as "=SUM(B2:B9)" or
    /// "=Costs!A3*2"). A leading apostrophe keeps a number as text. Empty
    /// clears the cell.
    pub input: String,
    /// The sheet's revision when you read this cell. If the cell (or its
    /// tab's rows and columns) changed since, the write is refused instead
    /// of overwriting it. Leave it out to write whatever is there.
    pub if_revision: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RowWrite {
    /// The tab, by name or ID. Defaults to the first.
    pub tab: Option<String>,
    /// The header whose value finds the row to change (such as "ID" or
    /// "Email"); the first row below the header with the same value is
    /// changed. Without it, or when no row matches, the row is added under
    /// the last one.
    pub key_column: Option<String>,
    /// The row's values by header in row 1: text (a formula starts with
    /// "="), numbers, true or false, or null to clear. A header that isn't
    /// there yet is added at the end of row 1.
    #[schemars(with = "Map<String, Value>")]
    pub values: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TabWrite {
    /// The tab to change, by name or ID. Leave it out with add to add one.
    pub tab: Option<String>,
    /// Add a tab with this name, at the end.
    pub add: Option<String>,
    /// Rename the tab; formulas that point at it follow.
    pub rename: Option<String>,
    /// true removes the tab and its cells (formulas that used it show
    /// #REF!). A sheet keeps at least one tab.
    pub remove: Option<bool>,
    /// Move the tab: 1 is first.
    pub position: Option<i32>,
    /// Rows at the top that stay put while scrolling (0 to 20).
    pub frozen_rows: Option<i32>,
    /// Insert empty rows before this row number; formulas follow.
    pub insert_rows_at: Option<i32>,
    /// Delete rows from this row number; formulas that used them show #REF!.
    pub delete_rows_at: Option<i32>,
    /// Insert empty columns before this column (such as "C").
    pub insert_columns_at: Option<String>,
    /// Delete columns from this column (such as "C").
    pub delete_columns_at: Option<String>,
    /// How many rows or columns to insert or delete (default 1).
    pub count: Option<i32>,
    /// Column widths in pixels by column letter (such as {"A": 160}); null
    /// puts one back to the default.
    #[schemars(with = "Option<Map<String, Value>>")]
    pub column_widths: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SortKey {
    /// A column of the range, by its letters (such as "C").
    pub column: String,
    /// Largest or last first.
    #[serde(default)]
    pub descending: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SortWrite {
    /// The tab, by name or ID. Defaults to the first.
    pub tab: Option<String>,
    /// The block whose rows are sorted, such as "A2:F40" (leave the header
    /// row out). Only its cells move; formulas that point at one follow it.
    pub range: String,
    /// One to three columns to sort by, most important first. Numbers come
    /// before text, empty cells last.
    pub by: Vec<SortKey>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConditionalFormatsWrite {
    /// The tab, by name or ID. Defaults to the first.
    pub tab: Option<String>,
    /// All of the tab's rules, in order: the first that applies to a cell
    /// wins. "when" is lessThan, greaterThan, atLeast, atMost, equals,
    /// notEquals (with "value"), between or notBetween (with "value" and
    /// "value2"), textContains, textDoesNotContain, textBeginsWith or
    /// textEndsWith (with "text"), empty, notEmpty, errors, formula (with
    /// "formula", such as "=$C2=\"Late\""), top or bottom (with "rank"),
    /// aboveAverage, belowAverage, duplicates, or colorScale (with "low" and
    /// "high" colours). "style" sets bold, italic, underline, strike,
    /// color and fill.
    pub rules: Vec<sheets::conditional::ConditionalRule>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FormatWrite {
    /// The tab, by name or ID. Defaults to the first.
    pub tab: Option<String>,
    /// The cells, in A1 form: "B2:B40", "C4", "B:D" (whole columns) or
    /// "1:1" (whole rows); up to 50,000 cells. Empty cells keep the format
    /// for what's typed in them later.
    pub range: String,
    /// A number format, as an Excel format code: "0.00", "#,##0", "0.0%",
    /// "$#,##0.00", "yyyy-mm-dd" or "@" (text). "" goes back to automatic.
    /// Typing "10%" or "$5" sets one by itself.
    pub number_format: Option<String>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// Text color as "#RRGGBB"; "" goes back to the default.
    pub color: Option<String>,
    /// Background as "#RRGGBB"; "" removes it.
    pub fill: Option<String>,
    /// "left", "center" or "right"; "" goes back to automatic.
    pub align: Option<String>,
    /// Wrap long text onto more lines.
    pub wrap: Option<bool>,
    /// true puts every format and style back to plain first.
    pub clear: Option<bool>,
}

impl FormatWrite {
    fn look(&self) -> sheets::LookInput {
        sheets::LookInput {
            number_format: self.number_format.clone(),
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            color: self.color.clone(),
            fill: self.fill.clone(),
            align: self.align.clone(),
            wrap: self.wrap,
            clear: self.clear,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SheetWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The sheet to change, by name or ID. Leave it out to create one
    /// (name is then required).
    pub sheet: Option<String>,
    /// The sheet's name: a new sheet's, or a new name for this one.
    pub name: Option<String>,
    /// What the sheet is for, on one line. Empty clears it.
    pub description: Option<String>,
    /// true archives the sheet (nothing is deleted); false restores it.
    pub archived: Option<bool>,
    /// Tab changes, applied first, in order.
    #[serde(default)]
    pub tabs: Vec<TabWrite>,
    /// Cells to set, in order (up to 2,000 changes in all).
    #[serde(default)]
    pub cells: Vec<CellWrite>,
    /// Rows to add or change by header, after the cells.
    #[serde(default)]
    pub rows: Vec<RowWrite>,
    /// Blocks to sort, after the rows. A sort moves cells, so edits that
    /// check ifRevision on the same tab go in a separate call.
    #[serde(default)]
    pub sorts: Vec<SortWrite>,
    /// Number formats and styles to set on blocks of cells, last. They
    /// don't change what's typed, values, or the cells' revisions.
    #[serde(default)]
    pub formats: Vec<FormatWrite>,
    /// Replace tabs' conditional formats: rules that colour cells by what
    /// they hold, such as {"range": "C2:C40", "when": "lessThan", "value": 0,
    /// "style": {"color": "#C62828"}}. An empty list removes a tab's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditional_formats: Vec<ConditionalFormatsWrite>,
    /// Fill a new sheet from a file instead of tabs, cells and rows: its
    /// tabs and what was typed in them, formulas included.
    pub import: Option<SheetImport>,
    /// A stable ID, so a retry doesn't apply the change twice.
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SheetImport {
    /// An Excel (.xlsx) or CSV file in this project, by ID (file_upload
    /// puts one here; file_list shows them). Up to 5 MB.
    pub file_id: Option<String>,
    /// CSV text, instead of a file: one row a line, fields split by commas
    /// (or tabs).
    pub csv: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChangedCellLine {
    pub tab: String,
    /// Such as "B12".
    pub cell: String,
    #[schemars(with = "Value")]
    pub value: Value,
    /// The formula, for cells that have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// The value as its number format shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// true when the cell was cleared or moved away.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cleared: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SheetWriteValue {
    /// The sheet's ID.
    pub id: String,
    pub name: String,
    /// The sheet's revision now: pass it as ifRevision next time.
    pub revision: i64,
    pub created: bool,
    pub archived: bool,
    /// Cells whose input changed (set, cleared or moved).
    pub cells_changed: usize,
    /// Cells whose computed value changed because of it.
    pub values_changed: usize,
    /// Cells whose number format or style changed.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub formats_changed: usize,
    /// Up to 200 cells that changed, the ones written first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<ChangedCellLine>,
    /// Set when your role only suggests on this sheet: nothing changed yet,
    /// and the write waits as this proposal for someone who edits the
    /// sheet (proposal_list, proposal_decide).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<String>,
    /// The cells the proposal would set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed: Vec<ProposedCellLine>,
    pub url: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idempotent_replay: bool,
}

/// A cell a proposal would set.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProposedCellLine {
    pub tab: String,
    /// Such as "B12".
    pub cell: String,
    /// Its input now; empty for an empty cell.
    pub before: String,
    /// The input proposed; empty clears it.
    pub after: String,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// The changes a call asks for, in order: tabs, then cells, then rows,
/// then formats.
fn changes_of(arguments: &SheetWriteArguments) -> Result<Vec<Change>, (String, String)> {
    let mut changes = Vec::new();
    for (index, tab) in arguments.tabs.iter().enumerate() {
        let at = |field: &str| format!("tabs[{index}].{field}");
        let count = tab.count.unwrap_or(1);
        if let Some(name) = &tab.add {
            if tab.tab.is_some() {
                return Err((
                    at("add"),
                    "Add a tab in an entry of its own, without tab.".to_owned(),
                ));
            }
            changes.push(Change::AddTab { name: name.clone() });
        }
        let reference = if tab.add.is_some() {
            tab.add.clone()
        } else {
            tab.tab.clone()
        };
        if let Some(name) = &tab.rename {
            changes.push(Change::RenameTab {
                tab: reference.clone(),
                name: name.clone(),
            });
        }
        let reference = tab.rename.clone().or(reference);
        if let Some(row) = tab.insert_rows_at {
            changes.push(Change::InsertRows {
                tab: reference.clone(),
                row,
                count,
            });
        }
        if let Some(row) = tab.delete_rows_at {
            changes.push(Change::DeleteRows {
                tab: reference.clone(),
                row,
                count,
            });
        }
        for (field, value, insert) in [
            ("insertColumnsAt", &tab.insert_columns_at, true),
            ("deleteColumnsAt", &tab.delete_columns_at, false),
        ] {
            if let Some(letters) = value {
                let Some(column) = address::column_number(letters.trim()) else {
                    return Err((
                        at(field),
                        "Name a column by its letters, such as \"C\".".to_owned(),
                    ));
                };
                changes.push(if insert {
                    Change::InsertColumns {
                        tab: reference.clone(),
                        column,
                        count,
                    }
                } else {
                    Change::DeleteColumns {
                        tab: reference.clone(),
                        column,
                        count,
                    }
                });
            }
        }
        if let Some(rows) = tab.frozen_rows {
            changes.push(Change::FreezeRows {
                tab: reference.clone(),
                rows,
            });
        }
        for (letters, width) in tab.column_widths.iter().flatten() {
            let Some(column) = address::column_number(letters.trim()) else {
                return Err((
                    at("columnWidths"),
                    format!(
                        "Name a column by its letters; \"{}\" isn't one.",
                        echo(letters)
                    ),
                ));
            };
            let width = match width {
                Value::Null => None,
                Value::Number(number) => {
                    match number.as_i64().and_then(|n| i32::try_from(n).ok()) {
                        Some(width) => Some(width),
                        None => {
                            return Err((
                                at("columnWidths"),
                                "Widths are whole pixels.".to_owned(),
                            ));
                        }
                    }
                }
                _ => {
                    return Err((
                        at("columnWidths"),
                        "Widths are whole pixels, or null.".to_owned(),
                    ));
                }
            };
            changes.push(Change::ColumnWidth {
                tab: reference.clone(),
                column,
                width,
            });
        }
        if let Some(position) = tab.position {
            changes.push(Change::MoveTab {
                tab: reference.clone(),
                position,
            });
        }
        if tab.remove == Some(true) {
            changes.push(Change::RemoveTab { tab: reference });
        }
    }
    for (index, cell) in arguments.cells.iter().enumerate() {
        let Some((row, column)) = address::cell(&cell.cell) else {
            return Err((
                format!("cells[{index}].cell"),
                format!(
                    "Name a cell such as \"B12\"; \"{}\" isn't one.",
                    echo(&cell.cell)
                ),
            ));
        };
        changes.push(Change::Cell {
            tab: cell.tab.clone(),
            row,
            column,
            input: cell.input.clone(),
            if_revision: cell.if_revision,
        });
    }
    for row in &arguments.rows {
        changes.push(Change::Row {
            tab: row.tab.clone(),
            key_column: row.key_column.clone(),
            values: row
                .values
                .iter()
                .map(|(header, value)| (header.clone(), value.clone()))
                .collect(),
        });
    }
    for (index, sort) in arguments.sorts.iter().enumerate() {
        let Some(range) = address::range(&sort.range, sheets::MAX_ROWS, sheets::MAX_COLUMNS) else {
            return Err((
                format!("sorts[{index}].range"),
                "Name the block in A1 form, such as \"A2:F40\".".to_owned(),
            ));
        };
        let mut by = Vec::with_capacity(sort.by.len());
        for (key, entry) in sort.by.iter().enumerate() {
            let Some(column) = address::column_number(entry.column.trim()) else {
                return Err((
                    format!("sorts[{index}].by[{key}].column"),
                    "Name a column by its letters, such as \"C\".".to_owned(),
                ));
            };
            by.push((column, entry.descending));
        }
        changes.push(Change::Sort {
            tab: sort.tab.clone(),
            rows: range.rows,
            columns: range.columns,
            by,
        });
    }
    for (index, format) in arguments.formats.iter().enumerate() {
        let Some(range) = address::range(&format.range, sheets::MAX_ROWS, sheets::MAX_COLUMNS)
        else {
            return Err((
                format!("formats[{index}].range"),
                "Name cells in A1 form: \"B2:B40\", \"C4\", \"B:D\" or \"1:1\".".to_owned(),
            ));
        };
        let patch = format
            .look()
            .patch()
            .map_err(|(field, message)| (format!("formats[{index}].{field}"), message))?;
        changes.push(Change::Format {
            tab: format.tab.clone(),
            rows: range.rows,
            columns: range.columns,
            patch,
        });
    }
    for (index, entry) in arguments.conditional_formats.iter().enumerate() {
        if let Err((rule, message)) = sheets::conditional::checked(entry.rules.clone()) {
            return Err((
                match rule {
                    Some(rule) => format!("conditionalFormats[{index}].rules[{rule}]"),
                    None => format!("conditionalFormats[{index}].rules"),
                },
                message,
            ));
        }
        changes.push(Change::ConditionalFormats {
            tab: entry.tab.clone(),
            rules: entry.rules.clone(),
        });
    }
    Ok(changes)
}

/// Where a change came from in the arguments, for messages.
fn change_field(arguments: &SheetWriteArguments, index: usize, changes: &[Change]) -> String {
    let cells_start = changes
        .iter()
        .position(|change| {
            matches!(
                change,
                Change::Cell { .. }
                    | Change::Row { .. }
                    | Change::Sort { .. }
                    | Change::Format { .. }
                    | Change::ConditionalFormats { .. }
            )
        })
        .unwrap_or(changes.len());
    if index < cells_start {
        return "tabs".to_owned();
    }
    let at = index - cells_start;
    if at < arguments.cells.len() {
        format!("cells[{at}]")
    } else if at < arguments.cells.len() + arguments.rows.len() {
        format!("rows[{}]", at - arguments.cells.len())
    } else if at < arguments.cells.len() + arguments.rows.len() + arguments.sorts.len() {
        format!(
            "sorts[{}]",
            at - arguments.cells.len() - arguments.rows.len()
        )
    } else if at
        < arguments.cells.len()
            + arguments.rows.len()
            + arguments.sorts.len()
            + arguments.formats.len()
    {
        format!(
            "formats[{}]",
            at - arguments.cells.len() - arguments.rows.len() - arguments.sorts.len()
        )
    } else {
        format!(
            "conditionalFormats[{}]",
            at - arguments.cells.len()
                - arguments.rows.len()
                - arguments.sorts.len()
                - arguments.formats.len()
        )
    }
}

fn changed_lines(applied: &Applied) -> Vec<ChangedCellLine> {
    applied
        .changed
        .iter()
        .map(|cell| ChangedCellLine {
            tab: cell.tab.clone(),
            cell: cell.address(),
            value: cell.value.to_json(),
            formula: cell
                .input
                .as_ref()
                .filter(|input| input.starts_with('='))
                .cloned(),
            display: cell.display.clone(),
            cleared: cell.input.is_none(),
        })
        .collect()
}

pub(super) async fn sheet_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: SheetWriteArguments,
) -> ToolResult<SheetWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let name = match arguments.name.as_deref() {
        Some(name) => match sheets::normalize_name(name) {
            Some(name) => Some(name),
            None => {
                return ToolResult::invalid(
                    "name",
                    "A sheet's name is one line of 1 to 80 characters.",
                );
            }
        },
        None => None,
    };
    let description = match arguments.description.as_deref() {
        Some(description) => match sheets::normalize_description(description) {
            Some(description) => Some(description),
            None => {
                return ToolResult::invalid(
                    "description",
                    "A description is one line of up to 500 characters.",
                );
            }
        },
        None => None,
    };
    if arguments.sheet.is_none() && name.is_none() {
        return ToolResult::invalid(
            "name",
            "Name the new sheet, or pass sheet to change one (sheet_list shows them).",
        );
    }
    let changes = match changes_of(&arguments) {
        Ok(changes) => changes,
        Err((field, message)) => return ToolResult::invalid(&field, message),
    };
    if changes.len() > sheets::MAX_CHANGES {
        return ToolResult::invalid(
            "cells",
            format!("Send up to {} changes at a time.", sheets::MAX_CHANGES),
        );
    }
    let Some(key) = request_key(arguments.client_request_id.as_deref()) else {
        return ToolResult::invalid("clientRequestId", REQUEST_ID_MESSAGE);
    };
    let imported = match &arguments.import {
        None => None,
        Some(import) => {
            if arguments.sheet.is_some() || !changes.is_empty() {
                return ToolResult::invalid(
                    "import",
                    "Import into a new sheet, on its own: leave out sheet, tabs, cells and rows.",
                );
            }
            match read_import(state, principal, import).await {
                Ok(Ok(tabs)) => Some(tabs),
                Ok(Err((field, message))) => return ToolResult::invalid(&field, message),
                Err(error) => return ToolResult::from_error(error, "sheet_write"),
            }
        }
    };
    let project_id = principal.project_id;
    let result = async {
        if module_off(state, principal).await? {
            return Ok(ToolResult::module_off("sheet_write", "Sheets"));
        }
        // Making, renaming, archiving or restoring a sheet checks per-project
        // counts and names under the project lock; cell and tab changes
        // lock only the sheet.
        let project_lock =
            arguments.sheet.is_none() || name.is_some() || arguments.archived.is_some();
        // Only cell contents of an existing sheet can be proposed (C1.11).
        let suggest_ok = arguments.sheet.is_some()
            && name.is_none()
            && description.is_none()
            && arguments.archived.is_none()
            && imported.is_none()
            && !changes.is_empty()
            && sheets::proposals::only_cells(&changes);
        let caller = if suggest_ok {
            let (_, limits) = super::caller_reach(state, principal).await?;
            Some(crate::access::Caller::of_principal(principal, limits))
        } else {
            None
        };
        let (mut transaction, edits) =
            match begin_write(state, principal, project_lock, suggest_ok).await? {
                Ok(started) => started,
                Err(refusal) => return Ok(refusal),
            };
        // Bound to the project, not the sheet: a retry finds its first answer
        // even after that write renamed the sheet it named.
        let scope = resource_bound_scope("mcp_sheets", project_id, "sheet_write")?;
        let request_hash = RequestHash::for_value(&arguments)?;
        let reservation = match reserve(
            &mut transaction,
            principal.actor_id,
            &scope,
            &key,
            request_hash,
        )
        .await
        {
            Ok(IdempotencyOutcome::Replay(replay)) => {
                let mut value = replay.decode::<SheetWriteValue>()?;
                transaction.commit().await?;
                value.idempotent_replay = true;
                return Ok(ToolResult::ok(value));
            }
            Ok(IdempotencyOutcome::Reserved(reservation)) => reservation,
            Err(AppError::Conflict("idempotency_key", "payload mismatch")) => {
                return Ok(ToolResult::invalid(
                    "clientRequestId",
                    "That clientRequestId was already used for a different change. Use a new one.",
                ));
            }
            Err(AppError::Conflict("idempotency_key", "command in progress")) => {
                return Ok(ToolResult::conflict(
                    "The same change is still being applied. Try again in a moment.",
                ));
            }
            Err(error) => return Err(error),
        };

        // The sheet: made, or found and locked, and renamed or archived.
        let mut touched = false;
        let (mut sheet, created) = match &arguments.sheet {
            None => {
                let name = name.clone().unwrap_or_default();
                let first_tab = sheets::FIRST_TAB_NAME;
                match sheets::create(
                    &mut transaction,
                    project_id,
                    principal.actor_id,
                    &name,
                    description.as_deref().unwrap_or_default(),
                    first_tab,
                )
                .await?
                {
                    CreateOutcome::Created(sheet) => (sheet, true),
                    CreateOutcome::NameTaken(existing) => {
                        return Ok(ToolResult::conflict(format!(
                            "A sheet called \"{}\" is already here (revision {}). Pass sheet: \"{}\" to change it, or pick another name.",
                            existing.name, existing.revision, existing.name
                        )));
                    }
                    CreateOutcome::ProjectFull => {
                        return Ok(ToolResult::conflict(format!(
                            "This project has as many sheets as it can keep ({} active). Archive one first.",
                            sheets::MAX_ACTIVE_SHEETS
                        )));
                    }
                }
            }
            Some(reference) => {
                let sheet_id = match find_sheet(&mut transaction, project_id, reference).await? {
                    Ok(sheet) => sheet.id,
                    Err(refusal) => return Ok(refusal),
                };
                let Some(sheet) = sheets::lock(&mut transaction, project_id, sheet_id).await?
                else {
                    return Ok(ToolResult::not_found("That sheet"));
                };
                if !edits {
                    // The role doesn't edit sheets: edit-own on their own
                    // sheet writes, suggest keeps it as a proposal.
                    let own = sqlx::query_scalar::<_, Option<Uuid>>(
                        "SELECT created_by_actor_id FROM sheets WHERE id = $1",
                    )
                    .bind(sheet.id)
                    .fetch_one(&mut *transaction)
                    .await?
                        == Some(principal.actor_id);
                    let caller = caller.as_ref().ok_or(AppError::Internal("sheet caller"))?;
                    let mode = crate::access::write_mode_locked(
                        &mut transaction,
                        caller,
                        "sheet",
                        project_id,
                        own,
                        Some("sheets"),
                    )
                    .await?
                    .ok()
                    .map(|grant| grant.mode);
                    match mode {
                        Some(crate::access::WriteMode::Direct) => {}
                        Some(crate::access::WriteMode::Propose) => {
                            return propose_cells(
                                state,
                                principal,
                                transaction,
                                &reservation,
                                &sheet,
                                &arguments,
                                &changes,
                            )
                            .await;
                        }
                        None => return Ok(ToolResult::denied(EDIT_MESSAGE)),
                    }
                }
                let change = SheetChange {
                    name: name.clone(),
                    description: description.clone(),
                    archived: arguments.archived,
                };
                match sheets::change(&mut transaction, &sheet, principal.actor_id, &change).await? {
                    ChangeOutcome::Changed(sheet) => {
                        touched = true;
                        (sheet, false)
                    }
                    ChangeOutcome::Unchanged(sheet) => (sheet, false),
                    ChangeOutcome::NameTaken(taken) => {
                        return Ok(ToolResult::conflict(format!(
                            "Another sheet here is called \"{}\". Pick another name.",
                            taken.name
                        )));
                    }
                    ChangeOutcome::ProjectFull => {
                        return Ok(ToolResult::conflict(format!(
                            "This project has as many active sheets as it can keep ({}). Archive one first.",
                            sheets::MAX_ACTIVE_SHEETS
                        )));
                    }
                }
            }
        };
        if sheet.archived && !changes.is_empty() {
            return Ok(ToolResult::conflict(format!(
                "\"{}\" is archived. Restore it (archived: false) to change its cells or tabs.",
                sheet.name
            )));
        }
        // A new sheet starts with one tab; a first entry that adds a tab
        // names that one instead of adding a second.
        let mut changes = match &imported {
            Some(tabs) => sheets::transfer::changes_for(sheets::FIRST_TAB_NAME, tabs),
            None => changes,
        };
        if created
            && let Some(Change::AddTab { name }) = changes.first().cloned()
        {
            changes[0] = Change::RenameTab {
                tab: None,
                name,
            };
        }
        let applied = if changes.is_empty() {
            None
        } else {
            let outcome = if imported.is_some() {
                sheets::apply_import(
                    &mut transaction,
                    &sheet,
                    principal.actor_id,
                    &changes,
                    MAX_ECHOED_CELLS,
                )
                .await?
            } else {
                sheets::apply(
                    &mut transaction,
                    &sheet,
                    principal.actor_id,
                    &changes,
                    MAX_ECHOED_CELLS,
                )
                .await?
            };
            match outcome {
                Ok(applied) => Some(applied),
                Err(problem) => {
                    let at = if imported.is_some() {
                        "import".to_owned()
                    } else {
                        change_field(&arguments, problem.index, &changes)
                    };
                    let message = format!("Nothing was written. {}", problem.message);
                    return Ok(match problem.kind {
                        ProblemKind::Invalid => ToolResult::invalid(&at, message),
                        ProblemKind::NotFound => ToolResult::NotFound { message },
                        ProblemKind::Conflict | ProblemKind::Full => {
                            ToolResult::conflict(message)
                        }
                    });
                }
            }
        };
        if let Some(applied) = &applied {
            touched |= applied.revision != sheet.revision;
            sheet.revision = applied.revision;
        }
        if created || touched {
            crate::outbox::enqueue_sheet_updated(
                &mut transaction,
                project_id,
                sheet.id,
                sheet.revision,
            )
            .await?;
        }
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            if created { "sheet.created" } else { "sheet.changed" },
            "sheet",
            sheet.id,
            json!({
                "revision": sheet.revision,
                "cells": applied.as_ref().map(|applied| applied.inputs_changed).unwrap_or(0),
            }),
        )
        .await?;
        let value = SheetWriteValue {
            id: sheet.id.to_string(),
            name: sheet.name.clone(),
            revision: sheet.revision,
            created,
            archived: sheet.archived,
            cells_changed: applied
                .as_ref()
                .map(|applied| applied.inputs_changed)
                .unwrap_or(0),
            values_changed: applied
                .as_ref()
                .map(|applied| applied.values_changed)
                .unwrap_or(0),
            formats_changed: applied
                .as_ref()
                .map(|applied| applied.looks_changed)
                .unwrap_or(0),
            changed: applied.as_ref().map(changed_lines).unwrap_or_default(),
            proposal_id: None,
            proposed: Vec::new(),
            url: sheet_url(state, project_id, sheet.id),
            idempotent_replay: false,
        };
        crate::idempotency::complete(
            &mut transaction,
            &reservation,
            axum::http::StatusCode::OK,
            &value,
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "sheet_write"))
}

/// A sheet_write by someone whose role only suggests on the sheet: kept
/// as a proposal (C1.11), answered and committed here.
async fn propose_cells(
    state: &AppState,
    principal: &McpPrincipal,
    mut transaction: Transaction<'static, Postgres>,
    reservation: &crate::idempotency::IdempotencyReservation,
    sheet: &Sheet,
    arguments: &SheetWriteArguments,
    changes: &[Change],
) -> Result<ToolResult<SheetWriteValue>, AppError> {
    use sheets::proposals::Proposed;
    let author = super::memory_tools::writer(principal);
    let (id, cells) =
        match sheets::proposals::propose(&mut transaction, author, sheet, changes, None).await? {
            Proposed::Made { id, cells } => (Some(id), cells),
            Proposed::Unchanged => (None, Vec::new()),
            Proposed::Refused(message) => return Ok(ToolResult::denied(message)),
            Proposed::Problem(problem) => {
                let at = change_field(arguments, problem.index, changes);
                let message = format!("Nothing was proposed. {}", problem.message);
                return Ok(match problem.kind {
                    ProblemKind::Invalid => ToolResult::invalid(&at, message),
                    ProblemKind::NotFound => ToolResult::NotFound { message },
                    ProblemKind::Conflict | ProblemKind::Full => ToolResult::conflict(message),
                });
            }
        };
    if let Some(id) = id {
        insert_audit(
            &mut transaction,
            principal,
            sheet.project_id,
            "sheet.proposal_created",
            "proposal",
            id,
            json!({"sheet": sheet.id, "cells": cells.len()}),
        )
        .await?;
    }
    let value = SheetWriteValue {
        id: sheet.id.to_string(),
        name: sheet.name.clone(),
        revision: sheet.revision,
        created: false,
        archived: sheet.archived,
        cells_changed: 0,
        values_changed: 0,
        formats_changed: 0,
        changed: Vec::new(),
        proposal_id: id.map(|id| id.to_string()),
        proposed: cells
            .iter()
            .take(MAX_ECHOED_CELLS)
            .map(|cell| ProposedCellLine {
                tab: cell.tab.clone(),
                cell: cell.address(),
                before: cell.before.clone(),
                after: cell.after.clone(),
            })
            .collect(),
        url: sheet_url(state, sheet.project_id, sheet.id),
        idempotent_replay: false,
    };
    crate::idempotency::complete(
        &mut transaction,
        reservation,
        axum::http::StatusCode::OK,
        &value,
    )
    .await?;
    transaction.commit().await?;
    Ok(ToolResult::ok(value))
}

/// The tabs an import asks for, read and checked, or the field and why not.
async fn read_import(
    state: &AppState,
    principal: &McpPrincipal,
    import: &SheetImport,
) -> Result<Result<Vec<sheets::engine::TabInput>, (String, String)>, AppError> {
    use sheets::transfer::{self, Format};
    let refuse = |field: &str, message: &str| Ok(Err((field.to_owned(), message.to_owned())));
    let (format, bytes) = match (&import.file_id, &import.csv) {
        (Some(_), Some(_)) | (None, None) => {
            return refuse("import", "Pass fileId or csv, one of them.");
        }
        (None, Some(text)) => (Format::Csv, text.as_bytes().to_vec()),
        (Some(file_id), None) => {
            let Some(file_id) = parse_id(file_id) else {
                return refuse("import.fileId", "Pass a file ID from file_list.");
            };
            let row = match crate::files::readable_file(
                &state.pool,
                super::file_tools::reader(principal),
                file_id,
            )
            .await
            {
                Ok(row) if row.project_id == principal.project_id => row,
                Ok(_) | Err(AppError::NotFound(_)) => {
                    return refuse(
                        "import.fileId",
                        "That file isn't in this project, or you can't see it.",
                    );
                }
                Err(error) => return Err(error),
            };
            let lower = row.name.to_lowercase();
            let format =
                if lower.ends_with(".xlsx") || row.content_type == Format::Xlsx.content_type() {
                    Format::Xlsx
                } else if lower.ends_with(".csv")
                    || lower.ends_with(".tsv")
                    || row.content_type.starts_with("text/")
                {
                    Format::Csv
                } else {
                    return refuse("import.fileId", "Import an Excel (.xlsx) or CSV file.");
                };
            if row.byte_size > transfer::MAX_IMPORT_BYTES as i64 {
                return refuse("import.fileId", &transfer::too_large());
            }
            let bytes = crate::files::FileStore::resolve(state, &row.store_id)
                .await?
                .read_all(&row.storage_key, transfer::MAX_IMPORT_BYTES as u64)
                .await?;
            (format, bytes)
        }
    };
    let read = tokio::task::spawn_blocking(move || {
        transfer::read(
            &sheets::engine::IronCalc,
            format,
            &bytes,
            sheets::FIRST_TAB_NAME,
        )
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "sheet import stopped");
        AppError::Internal("sheet import stopped")
    })?;
    Ok(read.map_err(|message| ("import".to_owned(), message)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(value: Value) -> SheetWriteArguments {
        serde_json::from_value(value).expect("ok")
    }

    #[test]
    fn changes_come_in_order_tabs_then_cells_then_rows() {
        let arguments = arguments(json!({
            "projectId": Uuid::nil().to_string(),
            "sheet": "Budget",
            "tabs": [
                {"add": "Costs", "frozenRows": 2},
                {"tab": "Sheet1", "rename": "Plan", "insertRowsAt": 3, "count": 2}
            ],
            "cells": [{"tab": "Costs", "cell": "B2", "input": "=1+1", "ifRevision": 4}],
            "rows": [{"keyColumn": "ID", "values": {"ID": 7, "Done": true}}]
        }));
        let changes = changes_of(&arguments).expect("ok");
        assert!(matches!(&changes[0], Change::AddTab { name } if name == "Costs"));
        assert!(
            matches!(&changes[1], Change::FreezeRows { tab: Some(tab), rows: 2 } if tab == "Costs")
        );
        assert!(
            matches!(&changes[2], Change::RenameTab { tab: Some(tab), name } if tab == "Sheet1" && name == "Plan")
        );
        assert!(
            matches!(&changes[3], Change::InsertRows { tab: Some(tab), row: 3, count: 2 } if tab == "Plan")
        );
        assert!(matches!(
            &changes[4],
            Change::Cell {
                row: 2,
                column: 2,
                if_revision: Some(4),
                ..
            }
        ));
        assert!(matches!(&changes[5], Change::Row { .. }));
        assert_eq!(change_field(&arguments, 4, &changes), "cells[0]");
        assert_eq!(change_field(&arguments, 5, &changes), "rows[0]");
        assert_eq!(change_field(&arguments, 1, &changes), "tabs");
    }

    #[test]
    fn bad_cells_are_named() {
        let arguments = arguments(json!({
            "projectId": Uuid::nil().to_string(),
            "sheet": "Budget",
            "cells": [{"cell": "12B", "input": "x"}]
        }));
        assert_eq!(
            changes_of(&arguments).expect_err("refused").0,
            "cells[0].cell"
        );
    }
}

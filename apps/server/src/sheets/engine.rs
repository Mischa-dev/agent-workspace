//! The formula engine behind sheets (AWD-54).
//!
//! Everything a sheet computes goes through [`FormulaEngine`], so the engine
//! can be swapped (or a linked Google sheet can compute instead) without
//! touching storage or tools. [`IronCalc`] is the one we use: the
//! `ironcalc_base` crate (MIT or Apache-2.0), which reads and writes Excel
//! formulas and recalculates a whole workbook.
//!
//! The engine is given a workbook as its cells' inputs, applies a list of
//! [`Operation`]s in order, recalculates, and hands back every cell's input
//! and value afterwards. Inputs are what a person types: `12`, `Due`,
//! `TRUE`, `=SUM(B2:B9)`, or `'00123` for text that looks like a number.
//! Inserting or deleting rows and columns, and renaming a tab, rewrite the
//! formulas that point at them, the way a spreadsheet does.
//!
//! It's synchronous and CPU-bound: callers run it on a blocking thread.

use std::collections::BTreeMap;

use ironcalc_base::{
    Model,
    cell::CellValue,
    expressions::types::Area,
    formatter::lexer::is_likely_date_number_format,
    number_format::to_excel_precision_str,
    types::{Alignment, CellType, Color, HorizontalAlignment, Style},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use super::conditional::ConditionalRule;

/// One tab's cells as they're stored: (row, column) from 1, and the input.
/// `looks` are the cells that aren't plain (a number format or a style),
/// which can include cells with nothing typed in them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabInput {
    pub name: String,
    pub cells: Vec<(i32, i32, String)>,
    pub looks: Vec<(i32, i32, Look)>,
    /// Its conditional formats, the first that applies winning (C1.17).
    pub rules: Vec<ConditionalRule>,
}

/// Horizontal alignment of a cell's text.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    Center,
    Right,
}

/// How a cell's text is drawn. Colors are `#RRGGBB`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CellStyle {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strike: bool,
    /// The text's color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The cell's background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    /// Long text wraps onto more lines instead of running over.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wrap: bool,
}

impl CellStyle {
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

/// A cell's number format (an Excel format code such as `0.0%`, `$#,##0`
/// or `yyyy-mm-dd`; None is automatic) and style.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Look {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "CellStyle::is_plain")]
    pub style: CellStyle,
}

impl Look {
    pub fn is_plain(&self) -> bool {
        self.format.is_none() && self.style.is_plain()
    }
}

/// A change to how cells look: each `Some` field is set (an inner `None`
/// puts it back to plain); with `clear`, everything else goes back first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LookPatch {
    pub clear: bool,
    pub format: Option<Option<String>>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub color: Option<Option<String>>,
    pub fill: Option<Option<String>>,
    pub align: Option<Option<Align>>,
    pub wrap: Option<bool>,
}

impl LookPatch {
    /// The patch that makes a cell look exactly like `look`.
    pub fn exactly(look: &Look) -> Self {
        let style = &look.style;
        Self {
            clear: true,
            format: Some(look.format.clone()),
            bold: Some(style.bold),
            italic: Some(style.italic),
            underline: Some(style.underline),
            strike: Some(style.strike),
            color: Some(style.color.clone()),
            fill: Some(style.fill.clone()),
            align: Some(style.align),
            wrap: Some(style.wrap),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn apply(&self, look: &Look) -> Look {
        let mut next = if self.clear {
            Look::default()
        } else {
            look.clone()
        };
        if let Some(format) = &self.format {
            next.format = format.clone();
        }
        let style = &mut next.style;
        for (field, value) in [
            (&mut style.bold, self.bold),
            (&mut style.italic, self.italic),
            (&mut style.underline, self.underline),
            (&mut style.strike, self.strike),
            (&mut style.wrap, self.wrap),
        ] {
            if let Some(value) = value {
                *field = value;
            }
        }
        if let Some(color) = &self.color {
            style.color = color.clone();
        }
        if let Some(fill) = &self.fill {
            style.fill = fill.clone();
        }
        if let Some(align) = self.align {
            style.align = align;
        }
        next
    }
}

/// `#RRGGBB` in capitals, or None for anything else.
pub fn color(value: &str) -> Option<String> {
    let value = value.trim();
    let hex = value.strip_prefix('#')?;
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_uppercase()))
}

fn color_of(value: &Color, plain: &str) -> Option<String> {
    match value {
        Color::Rgb(rgb) => color(rgb).filter(|rgb| rgb != plain),
        _ => None,
    }
}

/// How IronCalc's style for a cell reads here.
fn look_of(style: &Style) -> Look {
    let format = (!style.num_fmt.eq_ignore_ascii_case("general")).then(|| style.num_fmt.clone());
    let align = style
        .alignment
        .as_ref()
        .and_then(|alignment| match alignment.horizontal {
            HorizontalAlignment::Left => Some(Align::Left),
            HorizontalAlignment::Center | HorizontalAlignment::CenterContinuous => {
                Some(Align::Center)
            }
            HorizontalAlignment::Right => Some(Align::Right),
            _ => None,
        });
    Look {
        format,
        style: CellStyle {
            bold: style.font.b,
            italic: style.font.i,
            underline: style.font.u,
            strike: style.font.strike,
            // Black text and white fill are what a cell shows anyway; kept
            // as plain they follow the app's theme.
            color: color_of(&style.font.color, "#000000"),
            fill: color_of(&style.fill.color, "#FFFFFF"),
            align,
            wrap: style
                .alignment
                .as_ref()
                .is_some_and(|alignment| alignment.wrap_text),
        },
    }
}

/// `style` with `look` laid on it (its quote prefix and font kept).
fn styled(mut style: Style, look: &Look) -> Style {
    style.num_fmt = look.format.clone().unwrap_or_else(|| "general".to_owned());
    let cell = &look.style;
    style.font.b = cell.bold;
    style.font.i = cell.italic;
    style.font.u = cell.underline;
    style.font.strike = cell.strike;
    style.font.color = cell.color.clone().map(Color::Rgb).unwrap_or(Color::None);
    style.fill.color = cell.fill.clone().map(Color::Rgb).unwrap_or(Color::None);
    let horizontal = match cell.align {
        Some(Align::Left) => HorizontalAlignment::Left,
        Some(Align::Center) => HorizontalAlignment::Center,
        Some(Align::Right) => HorizontalAlignment::Right,
        None => HorizontalAlignment::General,
    };
    let mut alignment = style.alignment.take().unwrap_or_default();
    alignment.horizontal = horizontal;
    alignment.wrap_text = cell.wrap;
    style.alignment = (alignment != Alignment::default()).then_some(alignment);
    style
}

/// One change, applied in order. `tab` is the tab's index in the workbook
/// as it stands when the operation runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// Set a cell's input; empty clears it.
    Set {
        tab: usize,
        row: i32,
        column: i32,
        input: String,
    },
    InsertRows {
        tab: usize,
        row: i32,
        count: i32,
    },
    DeleteRows {
        tab: usize,
        row: i32,
        count: i32,
    },
    InsertColumns {
        tab: usize,
        column: i32,
        count: i32,
    },
    DeleteColumns {
        tab: usize,
        column: i32,
        count: i32,
    },
    /// Add a tab at the end.
    AddTab {
        name: String,
    },
    /// Rename a tab; formulas that point at it follow.
    RenameTab {
        tab: usize,
        name: String,
    },
    /// Remove a tab and its cells; formulas that pointed at it show #REF!.
    RemoveTab {
        tab: usize,
    },
    /// Sort the rows of a block by its columns' values, in order of the
    /// keys (column, descending). Rows move whole, the way moving a row
    /// does, so formulas that point at a moved row follow it.
    Sort {
        tab: usize,
        rows: (i32, i32),
        columns: (i32, i32),
        by: Vec<(i32, bool)>,
    },
    /// Change how a block of cells looks (rows and columns inclusive);
    /// inputs and values stay as they are.
    Format {
        tab: usize,
        rows: (i32, i32),
        columns: (i32, i32),
        patch: LookPatch,
    },
    /// Replace a tab's conditional formats.
    Rules {
        tab: usize,
        rules: Vec<ConditionalRule>,
    },
}

/// What a cell holds once computed.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Empty,
    Number(f64),
    Text(String),
    Boolean(bool),
    /// Such as `#DIV/0!` or `#REF!`.
    Error(String),
}

impl Value {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Number(_) => "number",
            Self::Text(_) => "text",
            Self::Boolean(_) => "boolean",
            Self::Error(_) => "error",
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Empty => serde_json::Value::Null,
            Self::Number(number) => serde_json::Number::from_f64(*number)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Self::Text(text) | Self::Error(text) => serde_json::Value::String(text.clone()),
            Self::Boolean(value) => serde_json::Value::Bool(*value),
        }
    }

    /// The value as a stored row reads it back.
    pub fn from_stored(kind: &str, value: &serde_json::Value) -> Self {
        match (kind, value) {
            ("number", serde_json::Value::Number(number)) => {
                Self::Number(number.as_f64().unwrap_or_default())
            }
            ("text", serde_json::Value::String(text)) => Self::Text(text.clone()),
            ("boolean", serde_json::Value::Bool(value)) => Self::Boolean(*value),
            ("error", serde_json::Value::String(text)) => Self::Error(text.clone()),
            _ => Self::Empty,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellOutput {
    pub row: i32,
    pub column: i32,
    /// Empty for a cell that only has a look.
    pub input: String,
    pub value: Value,
    pub look: Look,
    /// The value as its number format shows it, for a cell with a format.
    pub display: Option<String>,
    /// How it looks once its tab's conditional formats apply, when that
    /// differs from its own look.
    pub conditional: Option<CellStyle>,
}

/// One tab after the operations, its cells in row then column order.
#[derive(Clone, Debug, PartialEq)]
pub struct TabOutput {
    pub name: String,
    pub cells: Vec<CellOutput>,
    /// Each conditional format's range now, in the tab's order: inserting
    /// and deleting rows and columns moves them; None when its cells are
    /// all gone.
    pub rule_ranges: Vec<Option<String>>,
}

/// Why the engine refused. `operation` is the index of the operation that
/// failed, when one did.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineError {
    pub operation: Option<usize>,
    pub message: String,
}

pub trait FormulaEngine: Send + Sync {
    /// Apply `operations` to the workbook `tabs` and recalculate it.
    fn apply(
        &self,
        tabs: &[TabInput],
        operations: &[Operation],
    ) -> Result<Vec<TabOutput>, EngineError>;

    /// The workbook `tabs` as an Excel file (.xlsx), computed.
    fn to_xlsx(&self, tabs: &[TabInput]) -> Result<Vec<u8>, EngineError>;

    /// The tabs and cell inputs of an Excel file (.xlsx). `names` gives each
    /// tab the name it takes here, and formulas follow the new names. Text
    /// stays text. The caller checks the file's size first; the result can
    /// be as large as the file allows.
    fn read_xlsx(
        &self,
        bytes: &[u8],
        names: &dyn Fn(&[String]) -> Vec<String>,
    ) -> Result<Vec<TabInput>, EngineError>;
}

/// The IronCalc engine, in English, on UTC.
#[derive(Clone, Copy, Debug, Default)]
pub struct IronCalc;

const LOCALE: &str = "en";
const LANGUAGE: &str = "en";
const TIMEZONE: &str = "UTC";

fn whole(message: impl Into<String>) -> EngineError {
    EngineError {
        operation: None,
        message: message.into(),
    }
}

fn index(tab: usize, count: usize) -> Result<u32, String> {
    if tab < count {
        u32::try_from(tab).map_err(|_| "That tab doesn't exist.".to_owned())
    } else {
        Err("That tab doesn't exist.".to_owned())
    }
}

fn name_at(model: &Model<'_>, tab: u32) -> Result<String, String> {
    model
        .get_worksheets_properties()
        .get(tab as usize)
        .map(|properties| properties.name.clone())
        .ok_or_else(|| "That tab doesn't exist.".to_owned())
}

fn apply_one(model: &mut Model<'_>, operation: &Operation) -> Result<(), String> {
    let count = model.get_worksheets_properties().len();
    match operation {
        Operation::Set {
            tab,
            row,
            column,
            input,
        } => {
            let tab = index(*tab, count)?;
            if input.is_empty() {
                clear(model, tab, *row, *column)
            } else {
                model.set_user_input(tab, *row, *column, input.clone())
            }
        }
        Operation::Format {
            tab,
            rows,
            columns,
            patch,
        } => {
            let tab = index(*tab, count)?;
            for row in rows.0..=rows.1 {
                for column in columns.0..=columns.1 {
                    let style = model.get_style_for_cell(tab, row, column)?;
                    let look = patch.apply(&look_of(&style));
                    model.set_cell_style(tab, row, column, &styled(style, &look))?;
                }
            }
            Ok(())
        }
        Operation::InsertRows { tab, row, count: n } => {
            model.insert_rows(index(*tab, count)?, *row, *n)
        }
        Operation::DeleteRows { tab, row, count: n } => {
            model.delete_rows(index(*tab, count)?, *row, *n)
        }
        Operation::InsertColumns {
            tab,
            column,
            count: n,
        } => model.insert_columns(index(*tab, count)?, *column, *n),
        Operation::DeleteColumns {
            tab,
            column,
            count: n,
        } => model.delete_columns(index(*tab, count)?, *column, *n),
        Operation::AddTab { name } => model.add_sheet(name),
        Operation::RenameTab { tab, name } => {
            let old = name_at(model, index(*tab, count)?)?;
            if old == *name {
                Ok(())
            } else {
                model.rename_sheet(&old, name)
            }
        }
        Operation::RemoveTab { tab } => {
            if count <= 1 {
                return Err("A sheet keeps at least one tab.".to_owned());
            }
            model.delete_sheet(index(*tab, count)?)
        }
        Operation::Sort {
            tab,
            rows,
            columns,
            by,
        } => sort(model, index(*tab, count)?, *rows, *columns, by),
        Operation::Rules { tab, rules } => {
            let tab = index(*tab, count)?;
            while !model.get_conditional_formatting_list(tab)?.is_empty() {
                model.delete_conditional_formatting(tab, 0)?;
            }
            for (position, rule) in rules.iter().enumerate() {
                model
                    .add_conditional_formatting(tab, &rule.range, rule.input()?)
                    .map_err(|message| format!("Conditional format {}: {message}", position + 1))?;
            }
            Ok(())
        }
    }
}

/// How a cell looks once its tab's conditional formats apply, when that
/// differs from `own`.
fn conditional_of(
    model: &Model<'_>,
    tab: u32,
    row: i32,
    column: i32,
    own: &Style,
) -> Result<Option<CellStyle>, String> {
    let extended = model.get_extended_style_for_cell(tab, row, column)?;
    Ok((extended.style != *own).then(|| look_of(&extended.style).style))
}

/// Where a value sorts: numbers, then text (ignoring case), then true and
/// false, then errors, and empty cells last whichever way the sort goes.
fn sort_rank(value: &Value) -> (u8, f64, String) {
    match value {
        Value::Number(number) => (0, *number, String::new()),
        Value::Text(text) => (1, 0.0, text.to_lowercase()),
        Value::Boolean(value) => (2, f64::from(u8::from(*value)), String::new()),
        Value::Error(text) => (3, 0.0, text.clone()),
        Value::Empty => (4, 0.0, String::new()),
    }
}

fn compare(left: &Value, right: &Value, descending: bool) -> std::cmp::Ordering {
    let (left, right) = (sort_rank(left), sort_rank(right));
    // Empty cells stay last either way.
    if left.0 == 4 || right.0 == 4 {
        return left.0.cmp(&right.0);
    }
    let order = left
        .0
        .cmp(&right.0)
        .then(left.1.total_cmp(&right.1))
        .then_with(|| left.2.cmp(&right.2));
    if descending { order.reverse() } else { order }
}

fn sort(
    model: &mut Model<'_>,
    tab: u32,
    rows: (i32, i32),
    columns: (i32, i32),
    by: &[(i32, bool)],
) -> Result<(), String> {
    model.evaluate();
    let mut order = (rows.0..=rows.1)
        .map(|row| {
            let keys = by
                .iter()
                .map(|(column, _)| value_of(model, tab, row, *column))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((row, keys))
        })
        .collect::<Result<Vec<_>, String>>()?;
    order.sort_by(|(_, left), (_, right)| {
        by.iter()
            .zip(left.iter().zip(right))
            .map(|((_, descending), (left, right))| compare(left, right, *descending))
            .find(|order| order.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Where each row of the block goes.
    let mut moved_to = BTreeMap::new();
    for (place, (original, _)) in order.iter().enumerate() {
        moved_to.insert(*original, rows.0 + place as i32);
    }
    if moved_to.iter().all(|(from, to)| from == to) {
        return Ok(());
    }
    let names = model
        .get_worksheets_properties()
        .into_iter()
        .map(|properties| properties.name)
        .collect::<Vec<_>>();
    let sorted = names.get(tab as usize).cloned().unwrap_or_default();
    let block = Block {
        rows,
        columns,
        moved_to: &moved_to,
    };
    // Every cell's content and style, with references into the block's
    // cells pointed at where those cells go.
    let mut placed = Vec::new();
    for cell in model.get_all_cells() {
        let (sheet, row, column) = (cell.index, cell.row, cell.column);
        let in_block = sheet == tab && block.holds(row, column);
        let input = model.get_localized_cell_content(sheet, row, column)?;
        let rewritten = match input.strip_prefix('=') {
            Some(formula) => {
                let own = names.get(sheet as usize).map(String::as_str).unwrap_or("");
                follow(formula, own, &sorted, &block).map(|formula| format!("={formula}"))
            }
            None => None,
        };
        if in_block || rewritten.is_some() {
            let style = model.get_style_for_cell(sheet, row, column)?;
            let target = if in_block {
                moved_to.get(&row).copied().unwrap_or(row)
            } else {
                row
            };
            placed.push((sheet, target, column, rewritten.unwrap_or(input), style));
        }
    }
    model.range_clear_all(&Area {
        sheet: tab,
        row: rows.0,
        column: columns.0,
        width: columns.1 - columns.0 + 1,
        height: rows.1 - rows.0 + 1,
    })?;
    for (sheet, row, column, input, style) in placed {
        model.set_cell_style(sheet, row, column, &style)?;
        if input.is_empty() {
            continue;
        }
        model.set_user_input(sheet, row, column, input)?;
    }
    Ok(())
}

/// The block a sort reorders, and where each of its rows goes.
struct Block<'a> {
    rows: (i32, i32),
    columns: (i32, i32),
    moved_to: &'a BTreeMap<i32, i32>,
}

impl Block<'_> {
    fn holds(&self, row: i32, column: i32) -> bool {
        (self.rows.0..=self.rows.1).contains(&row)
            && (self.columns.0..=self.columns.1).contains(&column)
    }

    fn row(&self, row: i32) -> i32 {
        self.moved_to.get(&row).copied().unwrap_or(row)
    }
}

fn reference_text(column: i32, row: i32, absolute_column: bool, absolute_row: bool) -> String {
    format!(
        "{}{}{}{}",
        if absolute_column { "$" } else { "" },
        crate::sheets::address::column_name(column),
        if absolute_row { "$" } else { "" },
        row
    )
}

/// `formula` (without its `=`, in a cell of tab `own`) with every reference
/// to a cell of the block on tab `sorted` pointing where that cell went, or
/// None when nothing in it changes. A range moves only when it lies in one
/// row of the block; a range across rows keeps covering the same rows.
fn follow(formula: &str, own: &str, sorted: &str, block: &Block<'_>) -> Option<String> {
    use ironcalc_base::expressions::{
        lexer::{Lexer, LexerMode},
        token::TokenType,
    };
    let locale = ironcalc_base::locale::get_locale(LOCALE).ok()?;
    let language = ironcalc_base::language::get_language(LANGUAGE).ok()?;
    let chars = formula.chars().collect::<Vec<_>>();
    let mut lexer = Lexer::new(formula, LexerMode::A1, locale, language);
    let mut out = String::new();
    let mut copied = 0usize;
    let mut changed = false;
    let on_sorted =
        |sheet: &Option<String>| sheet.as_deref().unwrap_or(own).eq_ignore_ascii_case(sorted);
    loop {
        let before = lexer.get_position().max(0) as usize;
        let token = lexer.next_token();
        let after = lexer.get_position().max(0) as usize;
        let replacement = match &token {
            TokenType::EOF | TokenType::Illegal(_) => break,
            TokenType::Reference {
                sheet,
                row,
                column,
                absolute_column,
                absolute_row,
            } if on_sorted(sheet) && block.holds(*row, *column) && block.row(*row) != *row => Some(
                reference_text(*column, block.row(*row), *absolute_column, *absolute_row),
            ),
            TokenType::Range { sheet, left, right }
                if on_sorted(sheet)
                    && left.row == right.row
                    && block.holds(left.row, left.column)
                    && block.holds(right.row, right.column)
                    && block.row(left.row) != left.row =>
            {
                let row = block.row(left.row);
                Some(format!(
                    "{}:{}",
                    reference_text(left.column, row, left.absolute_column, left.absolute_row),
                    reference_text(right.column, row, right.absolute_column, right.absolute_row)
                ))
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            let text = chars[before.min(chars.len())..after.min(chars.len())]
                .iter()
                .collect::<String>();
            let start = before + (text.len() - text.trim_start().len());
            let token_text = &chars[start.min(chars.len())..after.min(chars.len())]
                .iter()
                .collect::<String>();
            // Keep the sheet name as written; change only the cell part.
            let prefix = token_text
                .rfind('!')
                .map(|bang| &token_text[..=bang])
                .unwrap_or("");
            out.extend(&chars[copied..start]);
            out.push_str(prefix);
            out.push_str(&replacement);
            copied = after;
            changed = true;
        }
        if after >= chars.len() {
            break;
        }
    }
    if !changed {
        return None;
    }
    out.extend(&chars[copied..]);
    Some(out)
}

/// Empties a cell and keeps its look, the way a spreadsheet's Delete does.
fn clear(model: &mut Model<'_>, tab: u32, row: i32, column: i32) -> Result<(), String> {
    model.range_clear_contents(&Area {
        sheet: tab,
        row,
        column,
        width: 1,
        height: 1,
    })?;
    let style = model.get_style_for_cell(tab, row, column)?;
    if style.quote_prefix {
        let mut style = style;
        style.quote_prefix = false;
        model.set_cell_style(tab, row, column, &style)?;
    }
    Ok(())
}

/// What a cell holds as it would be typed. A plain number under a date
/// format reads as the date, as long as typing that date back gives the
/// same number; otherwise as the number.
fn input_of(
    model: &Model<'_>,
    scratch: &mut Model<'_>,
    tab: u32,
    row: i32,
    column: i32,
) -> Result<String, String> {
    let input = model.get_localized_cell_content(tab, row, column)?;
    let style = model.get_style_for_cell(tab, row, column)?;
    if input.starts_with('=') || !is_likely_date_number_format(&style.num_fmt) {
        return Ok(input);
    }
    let Ok(CellValue::Number(number)) = model.get_cell_value_by_index(tab, row, column) else {
        return Ok(input);
    };
    let probe = Style {
        num_fmt: style.num_fmt.clone(),
        ..Style::default()
    };
    scratch.set_cell_style(0, 1, 1, &probe)?;
    scratch.set_user_input(0, 1, 1, input.clone())?;
    match scratch.get_cell_value_by_index(0, 1, 1) {
        Ok(CellValue::Number(back)) if back == number => Ok(input),
        _ => Ok(to_excel_precision_str(number)),
    }
}

fn value_of(model: &Model<'_>, tab: u32, row: i32, column: i32) -> Result<Value, String> {
    let kind = model.get_cell_type(tab, row, column)?;
    let value = model.get_cell_value_by_index(tab, row, column)?;
    Ok(match (kind, value) {
        (_, CellValue::None) => Value::Empty,
        (CellType::ErrorValue, CellValue::String(text)) => Value::Error(text),
        (_, CellValue::String(text)) => Value::Text(text),
        (_, CellValue::Number(number)) => Value::Number(number),
        (_, CellValue::Boolean(value)) => Value::Boolean(value),
    })
}

/// A model holding `tabs`, not yet evaluated.
fn build(tabs: &[TabInput]) -> Result<Model<'static>, EngineError> {
    let Some(first) = tabs.first() else {
        return Err(whole("A sheet keeps at least one tab."));
    };
    let mut model = Model::new_empty("sheet", LOCALE, TIMEZONE, LANGUAGE).map_err(whole)?;
    model.rename_sheet("Sheet1", &first.name).map_err(whole)?;
    for tab in &tabs[1..] {
        model.add_sheet(&tab.name).map_err(whole)?;
    }
    for (tab_index, tab) in tabs.iter().enumerate() {
        let tab_index = u32::try_from(tab_index).map_err(|_| whole("Too many tabs."))?;
        // Looks first, so an input under a date format reads as that date.
        for (row, column, look) in &tab.looks {
            let style = model
                .get_style_for_cell(tab_index, *row, *column)
                .map_err(whole)?;
            model
                .set_cell_style(tab_index, *row, *column, &styled(style, look))
                .map_err(whole)?;
        }
        for (row, column, input) in &tab.cells {
            model
                .set_user_input(tab_index, *row, *column, input.clone())
                .map_err(whole)?;
        }
        // A kept rule that no longer reads (a formula the engine has since
        // refused) is left out rather than stopping the whole sheet.
        for rule in &tab.rules {
            if let Ok(input) = rule.input() {
                let _ = model.add_conditional_formatting(tab_index, &rule.range, input);
            }
        }
    }
    Ok(model)
}

impl FormulaEngine for IronCalc {
    fn apply(
        &self,
        tabs: &[TabInput],
        operations: &[Operation],
    ) -> Result<Vec<TabOutput>, EngineError> {
        let mut model = build(tabs)?;
        for (position, operation) in operations.iter().enumerate() {
            apply_one(&mut model, operation).map_err(|message| EngineError {
                operation: Some(position),
                message,
            })?;
        }
        model.evaluate();

        let names = model
            .get_worksheets_properties()
            .into_iter()
            .map(|properties| properties.name)
            .collect::<Vec<_>>();
        let mut cells: BTreeMap<u32, Vec<CellOutput>> = BTreeMap::new();
        let mut scratch = Model::new_empty("scratch", LOCALE, TIMEZONE, LANGUAGE).map_err(whole)?;
        for cell in model.get_all_cells() {
            let (tab, row, column) = (cell.index, cell.row, cell.column);
            let style = model.get_style_for_cell(tab, row, column).map_err(whole)?;
            let look = look_of(&style);
            let input = input_of(&model, &mut scratch, tab, row, column).map_err(whole)?;
            if input.is_empty() && look.is_plain() {
                continue;
            }
            let value = if input.is_empty() {
                Value::Empty
            } else {
                value_of(&model, tab, row, column).map_err(whole)?
            };
            let display = match (&look.format, &value) {
                (Some(_), Value::Number(_)) => Some(
                    model
                        .get_formatted_cell_value(tab, row, column)
                        .map_err(whole)?,
                ),
                _ => None,
            };
            let conditional = conditional_of(&model, tab, row, column, &style).map_err(whole)?;
            cells.entry(tab).or_default().push(CellOutput {
                row,
                column,
                input,
                value,
                look,
                display,
                conditional,
            });
        }
        Ok(names
            .into_iter()
            .enumerate()
            .map(|(position, name)| {
                let mut tab_cells = cells.remove(&(position as u32)).unwrap_or_default();
                tab_cells.sort_by_key(|cell| (cell.row, cell.column));
                let mut rule_ranges = model
                    .get_conditional_formatting_list(position as u32)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|view| (view.index, view.range))
                    .collect::<Vec<_>>();
                rule_ranges.sort_by_key(|(index, _)| *index);
                TabOutput {
                    name,
                    cells: tab_cells,
                    rule_ranges: rule_ranges
                        .into_iter()
                        .map(|(_, range)| (!range.trim().is_empty()).then_some(range))
                        .collect(),
                }
            })
            .collect())
    }

    fn to_xlsx(&self, tabs: &[TabInput]) -> Result<Vec<u8>, EngineError> {
        let mut model = build(tabs)?;
        model.evaluate();
        let writer =
            ironcalc::export::save_xlsx_to_writer(&model, std::io::Cursor::new(Vec::new()))
                .map_err(|error| whole(format!("The Excel file couldn't be written: {error}")))?;
        Ok(writer.into_inner())
    }

    fn read_xlsx(
        &self,
        bytes: &[u8],
        names: &dyn Fn(&[String]) -> Vec<String>,
    ) -> Result<Vec<TabInput>, EngineError> {
        let unreadable = |_| whole("That isn't an Excel file (.xlsx) this can read.");
        let workbook = ironcalc::import::load_from_xlsx_bytes(bytes, "import", LOCALE, TIMEZONE)
            .map_err(unreadable)?;
        let mut model = Model::from_workbook(workbook, LANGUAGE)
            .map_err(|_| whole("That isn't an Excel file (.xlsx) this can read."))?;
        let original = model
            .get_worksheets_properties()
            .into_iter()
            .map(|properties| properties.name)
            .collect::<Vec<_>>();
        let wanted = names(&original);
        // Rename through the model so formulas follow; a pass through
        // placeholder names lets two tabs swap names.
        if wanted != original {
            let taken = original
                .iter()
                .chain(&wanted)
                .map(|name| name.to_lowercase())
                .collect::<std::collections::HashSet<_>>();
            let mut next = 0;
            for index in 0..original.len() {
                let placeholder = loop {
                    let candidate = format!("~import {next}");
                    next += 1;
                    if !taken.contains(&candidate) {
                        break candidate;
                    }
                };
                model
                    .rename_sheet_by_index(index as u32, &placeholder)
                    .map_err(whole)?;
            }
            for (index, name) in wanted.iter().enumerate() {
                model
                    .rename_sheet_by_index(index as u32, name)
                    .map_err(whole)?;
            }
        }
        let mut tabs = wanted
            .into_iter()
            .map(|name| TabInput {
                name,
                ..TabInput::default()
            })
            .collect::<Vec<_>>();
        let mut scratch = Model::new_empty("scratch", LOCALE, TIMEZONE, LANGUAGE).map_err(whole)?;
        let mut dates = Model::new_empty("dates", LOCALE, TIMEZONE, LANGUAGE).map_err(whole)?;
        for cell in model.get_all_cells() {
            let look = look_of(
                &model
                    .get_style_for_cell(cell.index, cell.row, cell.column)
                    .map_err(whole)?,
            );
            if !look.is_plain()
                && let Some(tab) = tabs.get_mut(cell.index as usize)
            {
                tab.looks.push((cell.row, cell.column, look));
            }
            let mut input =
                input_of(&model, &mut dates, cell.index, cell.row, cell.column).map_err(whole)?;
            if input.is_empty() {
                continue;
            }
            let formula = model
                .get_cell_formula(cell.index, cell.row, cell.column)
                .map_err(whole)?
                .is_some();
            let text = matches!(
                model.get_cell_type(cell.index, cell.row, cell.column),
                Ok(CellType::Text)
            );
            // Text such as 00123 or =x would read back as a number or a
            // formula, so it keeps a leading apostrophe.
            if !formula && text && !input.starts_with('\'') && !stays_text(&mut scratch, &input)? {
                input.insert(0, '\'');
            }
            if let Some(tab) = tabs.get_mut(cell.index as usize) {
                tab.cells.push((cell.row, cell.column, input));
            }
        }
        for tab in &mut tabs {
            tab.cells.sort_by_key(|(row, column, _)| (*row, *column));
            tab.looks.sort_by_key(|(row, column, _)| (*row, *column));
        }
        Ok(tabs)
    }
}

/// Whether `input` typed into a cell reads as text.
fn stays_text(scratch: &mut Model, input: &str) -> Result<bool, EngineError> {
    scratch
        .set_user_input(0, 1, 1, input.to_owned())
        .map_err(whole)?;
    let formula = scratch.get_cell_formula(0, 1, 1).map_err(whole)?.is_some();
    Ok(!formula && matches!(scratch.get_cell_type(0, 1, 1), Ok(CellType::Text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(name: &str, cells: &[(i32, i32, &str)]) -> TabInput {
        TabInput {
            name: name.to_owned(),
            cells: cells
                .iter()
                .map(|(row, column, input)| (*row, *column, (*input).to_owned()))
                .collect(),
            looks: Vec::new(),
            rules: Vec::new(),
        }
    }

    fn cell(output: &[TabOutput], tab: usize, row: i32, column: i32) -> Option<&CellOutput> {
        output[tab]
            .cells
            .iter()
            .find(|cell| cell.row == row && cell.column == column)
    }

    #[test]
    fn formulas_compute_across_tabs() {
        let output = IronCalc
            .apply(
                &[
                    tab("Budget", &[(1, 1, "Item"), (2, 1, "4"), (3, 1, "6")]),
                    tab("Summary", &[(1, 1, "=SUM(Budget!A2:A3)")]),
                ],
                &[Operation::Set {
                    tab: 0,
                    row: 4,
                    column: 1,
                    input: "=A2*A3".to_owned(),
                }],
            )
            .expect("ok");
        assert_eq!(
            cell(&output, 1, 1, 1).expect("ok").value,
            Value::Number(10.0)
        );
        assert_eq!(
            cell(&output, 0, 4, 1).expect("ok").value,
            Value::Number(24.0)
        );
        assert_eq!(
            cell(&output, 0, 1, 1).expect("ok").value,
            Value::Text("Item".to_owned())
        );
    }

    #[test]
    fn inputs_round_trip_and_errors_are_values() {
        let output = IronCalc
            .apply(
                &[tab(
                    "Data",
                    &[(1, 1, "'00123"), (1, 2, "TRUE"), (1, 3, "=1/0"), (1, 4, "")],
                )],
                &[],
            )
            .expect("ok");
        assert_eq!(cell(&output, 0, 1, 1).expect("ok").input, "'00123");
        assert_eq!(
            cell(&output, 0, 1, 1).expect("ok").value,
            Value::Text("00123".to_owned())
        );
        assert_eq!(
            cell(&output, 0, 1, 2).expect("ok").value,
            Value::Boolean(true)
        );
        assert_eq!(cell(&output, 0, 1, 3).expect("ok").input, "=1/0");
        assert!(matches!(
            cell(&output, 0, 1, 3).expect("ok").value,
            Value::Error(_)
        ));
        assert!(cell(&output, 0, 1, 4).is_none());
    }

    #[test]
    fn inserting_rows_and_renaming_tabs_rewrite_formulas() {
        let output = IronCalc
            .apply(
                &[
                    tab("Costs", &[(1, 1, "5"), (2, 1, "7"), (3, 1, "=SUM(A1:A2)")]),
                    tab("Total", &[(1, 1, "=Costs!A3")]),
                ],
                &[
                    Operation::InsertRows {
                        tab: 0,
                        row: 2,
                        count: 1,
                    },
                    Operation::Set {
                        tab: 0,
                        row: 2,
                        column: 1,
                        input: "100".to_owned(),
                    },
                    Operation::RenameTab {
                        tab: 0,
                        name: "Spend".to_owned(),
                    },
                ],
            )
            .expect("ok");
        assert_eq!(output[0].name, "Spend");
        assert_eq!(cell(&output, 0, 4, 1).expect("ok").input, "=SUM(A1:A3)");
        assert_eq!(
            cell(&output, 0, 4, 1).expect("ok").value,
            Value::Number(112.0)
        );
        assert_eq!(cell(&output, 1, 1, 1).expect("ok").input, "=Spend!A4");
        assert_eq!(
            cell(&output, 1, 1, 1).expect("ok").value,
            Value::Number(112.0)
        );
    }

    #[test]
    fn a_failed_operation_names_itself_and_the_last_tab_stays() {
        let error = IronCalc
            .apply(&[tab("Only", &[])], &[Operation::RemoveTab { tab: 0 }])
            .expect_err("refused");
        assert_eq!(error.operation, Some(0));
        let error = IronCalc
            .apply(
                &[tab("Only", &[])],
                &[Operation::Set {
                    tab: 3,
                    row: 1,
                    column: 1,
                    input: "1".to_owned(),
                }],
            )
            .expect_err("refused");
        assert_eq!(error.operation, Some(0));
    }

    #[test]
    fn an_excel_file_round_trips_its_tabs_and_formulas() {
        let tabs = vec![
            tab(
                "Costs",
                &[
                    (1, 1, "Item"),
                    (1, 2, "Cost"),
                    (2, 1, "Rent"),
                    (2, 2, "1200"),
                    (3, 2, "=SUM(B2:B2)"),
                ],
            ),
            tab("Notes", &[(1, 1, "'00123"), (2, 1, "=Costs!B3*2")]),
        ];
        let bytes = IronCalc.to_xlsx(&tabs).expect("written");
        let back = IronCalc
            .read_xlsx(&bytes, &|names| names.to_vec())
            .expect("read");
        assert_eq!(
            back.iter().map(|tab| tab.name.as_str()).collect::<Vec<_>>(),
            ["Costs", "Notes"]
        );
        assert!(back[0].cells.contains(&(3, 2, "=SUM(B2:B2)".to_owned())));
        assert!(back[1].cells.contains(&(2, 1, "=Costs!B3*2".to_owned())));
        assert_eq!(
            IronCalc.apply(&back, &[]).expect("computed")[1].cells[1].value,
            Value::Number(2400.0)
        );
        assert!(
            IronCalc
                .read_xlsx(b"not a zip", &|names| names.to_vec())
                .is_err()
        );
    }

    #[test]
    fn imported_text_that_reads_as_something_else_is_quoted() {
        let mut scratch = Model::new_empty("scratch", LOCALE, TIMEZONE, LANGUAGE).expect("model");
        for typed in ["00123", "=A1", "TRUE", "-5", "#N/A"] {
            assert!(!stays_text(&mut scratch, typed).expect("typed"), "{typed}");
        }
        for typed in ["Rent", "A1:B2", "Q1 total"] {
            assert!(stays_text(&mut scratch, typed).expect("typed"), "{typed}");
        }
    }

    #[test]
    fn removing_a_tab_leaves_ref_errors() {
        let output = IronCalc
            .apply(
                &[tab("A", &[(1, 1, "3")]), tab("B", &[(1, 1, "=A!A1+1")])],
                &[Operation::RemoveTab { tab: 0 }],
            )
            .expect("ok");
        assert_eq!(output.len(), 1);
        assert!(matches!(
            cell(&output, 0, 1, 1).expect("ok").value,
            Value::Error(_)
        ));
    }

    fn format(tab: usize, rows: (i32, i32), columns: (i32, i32), patch: LookPatch) -> Operation {
        Operation::Format {
            tab,
            rows,
            columns,
            patch,
        }
    }

    #[test]
    fn a_typed_percent_keeps_its_format_and_computes_as_a_fraction() {
        let output = IronCalc
            .apply(&[tab("Rates", &[(1, 1, "10%"), (2, 1, "=A1*200")])], &[])
            .expect("ok");
        let rate = cell(&output, 0, 1, 1).expect("rate");
        assert_eq!(rate.value, Value::Number(0.1));
        assert_eq!(rate.look.format.as_deref(), Some("#,##0%"));
        assert_eq!(rate.display.as_deref(), Some("10%"));
        assert_eq!(
            cell(&output, 0, 2, 1).expect("total").value,
            Value::Number(20.0)
        );
        // Stored and built again, it reads the same.
        let stored = TabInput {
            name: "Rates".to_owned(),
            cells: vec![(1, 1, rate.input.clone())],
            looks: vec![(1, 1, rate.look.clone())],
            rules: Vec::new(),
        };
        let again = IronCalc.apply(&[stored], &[]).expect("ok");
        assert_eq!(again[0].cells[0], *rate);
    }

    #[test]
    fn formatting_a_block_reaches_empty_cells_and_survives_a_clear() {
        let output = IronCalc
            .apply(
                &[tab("Data", &[(1, 1, "Name"), (2, 2, "0.25")])],
                &[
                    format(
                        0,
                        (1, 1),
                        (1, 2),
                        LookPatch {
                            bold: Some(true),
                            fill: Some(Some("#FFEEAA".to_owned())),
                            align: Some(Some(Align::Center)),
                            ..LookPatch::default()
                        },
                    ),
                    format(
                        0,
                        (2, 3),
                        (2, 2),
                        LookPatch {
                            format: Some(Some("0.0%".to_owned())),
                            ..LookPatch::default()
                        },
                    ),
                    Operation::Set {
                        tab: 0,
                        row: 1,
                        column: 1,
                        input: String::new(),
                    },
                ],
            )
            .expect("ok");
        let header = cell(&output, 0, 1, 1).expect("cleared but styled");
        assert_eq!(header.input, "");
        assert_eq!(header.value, Value::Empty);
        assert!(header.look.style.bold);
        assert_eq!(header.look.style.fill.as_deref(), Some("#FFEEAA"));
        assert_eq!(header.look.style.align, Some(Align::Center));
        assert!(
            cell(&output, 0, 1, 2)
                .expect("empty, styled")
                .look
                .style
                .bold
        );
        let share = cell(&output, 0, 2, 2).expect("share");
        assert_eq!(share.display.as_deref(), Some("25.0%"));
        assert_eq!(share.input, "0.25");
        assert!(cell(&output, 0, 3, 2).is_some_and(|cell| cell.input.is_empty()));
        // Clearing the look leaves nothing.
        let plain = IronCalc
            .apply(
                &[TabInput {
                    name: "Data".to_owned(),
                    cells: vec![],
                    looks: vec![(1, 1, header.look.clone())],
                    rules: Vec::new(),
                }],
                &[format(
                    0,
                    (1, 1),
                    (1, 1),
                    LookPatch {
                        clear: true,
                        ..LookPatch::default()
                    },
                )],
            )
            .expect("ok");
        assert!(plain[0].cells.is_empty());
    }

    #[test]
    fn dates_read_back_as_typed() {
        let output = IronCalc
            .apply(&[tab("Due", &[(1, 1, "2026-11-18")])], &[])
            .expect("ok");
        let due = cell(&output, 0, 1, 1).expect("due");
        assert!(matches!(due.value, Value::Number(_)));
        assert!(due.look.format.is_some());
        let again = IronCalc
            .apply(
                &[TabInput {
                    name: "Due".to_owned(),
                    cells: vec![(1, 1, due.input.clone())],
                    looks: vec![(1, 1, due.look.clone())],
                    rules: Vec::new(),
                }],
                &[],
            )
            .expect("ok");
        assert_eq!(again[0].cells[0], *due);
    }

    #[test]
    fn an_excel_file_keeps_formats_and_styles() {
        let look = Look {
            format: Some("$#,##0.00".to_owned()),
            style: CellStyle {
                bold: true,
                italic: true,
                color: Some("#AA0000".to_owned()),
                fill: Some("#EEFFEE".to_owned()),
                align: Some(Align::Right),
                wrap: true,
                ..CellStyle::default()
            },
        };
        let tabs = vec![TabInput {
            name: "Costs".to_owned(),
            cells: vec![(1, 1, "1200".to_owned()), (2, 1, "10%".to_owned())],
            looks: vec![(1, 1, look.clone())],
            rules: Vec::new(),
        }];
        let bytes = IronCalc.to_xlsx(&tabs).expect("written");
        let back = IronCalc
            .read_xlsx(&bytes, &|names| names.to_vec())
            .expect("read");
        assert_eq!(back[0].looks[0], (1, 1, look));
        assert_eq!(back[0].looks[1].2.format.as_deref(), Some("#,##0%"));
        let computed = IronCalc.apply(&back, &[]).expect("computed");
        assert_eq!(computed[0].cells[0].display.as_deref(), Some("$1,200.00"));
        assert_eq!(computed[0].cells[1].display.as_deref(), Some("10%"));
    }

    #[test]
    fn a_less_than_zero_rule_colours_the_right_cells_after_an_edit() {
        let rule: ConditionalRule = serde_json::from_value(serde_json::json!({
            "range": "B1:B3", "when": "lessThan", "value": 0, "style": {"color": "#C62828"}
        }))
        .expect("rule");
        let mut sheet = tab("Money", &[(1, 2, "5"), (2, 2, "-2"), (3, 2, "=B1-10")]);
        sheet.rules = vec![rule.clone()];
        let red = |output: &[TabOutput], row| {
            cell(output, 0, row, 2)
                .and_then(|cell| cell.conditional.clone())
                .and_then(|style| style.color)
        };
        let output = IronCalc.apply(&[sheet.clone()], &[]).expect("apply");
        assert_eq!(red(&output, 1), None);
        assert_eq!(red(&output, 2).as_deref(), Some("#C62828"));
        assert_eq!(
            red(&output, 3).as_deref(),
            Some("#C62828"),
            "a formula's value counts"
        );
        assert_eq!(output[0].rule_ranges, vec![Some("B1:B3".to_owned())]);

        let edited = IronCalc
            .apply(
                &[sheet.clone()],
                &[Operation::Set {
                    tab: 0,
                    row: 1,
                    column: 2,
                    input: "20".to_owned(),
                }],
            )
            .expect("edit");
        assert_eq!(red(&edited, 1), None);
        assert_eq!(red(&edited, 3), None, "B3 is 10 now");
        assert_eq!(red(&edited, 2).as_deref(), Some("#C62828"));

        // A row inserted above moves the rule with its cells.
        let moved = IronCalc
            .apply(
                &[sheet.clone()],
                &[Operation::InsertRows {
                    tab: 0,
                    row: 1,
                    count: 1,
                }],
            )
            .expect("insert");
        assert_eq!(moved[0].rule_ranges, vec![Some("B2:B4".to_owned())]);
        assert_eq!(red(&moved, 3).as_deref(), Some("#C62828"));

        // Replacing the rules: none left, nothing coloured.
        let cleared = IronCalc
            .apply(
                &[sheet],
                &[Operation::Rules {
                    tab: 0,
                    rules: Vec::new(),
                }],
            )
            .expect("clear rules");
        assert_eq!(red(&cleared, 2), None);
        assert!(cleared[0].rule_ranges.is_empty());

        // And an .xlsx carries the rule.
        let mut with_rule = tab("Money", &[(1, 2, "-1")]);
        with_rule.rules = vec![rule];
        let bytes = IronCalc.to_xlsx(&[with_rule]).expect("xlsx");
        let workbook = ironcalc::import::load_from_xlsx_bytes(&bytes, "check", LOCALE, TIMEZONE)
            .expect("read back");
        let model = Model::from_workbook(workbook, LANGUAGE).expect("model");
        let rules = model.get_conditional_formatting_list(0).expect("rules");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].range, "B1:B3");
    }

    #[test]
    fn sorting_moves_rows_and_formulas_follow_them() {
        let output = IronCalc
            .apply(
                &[
                    tab(
                        "Team",
                        &[
                            (1, 1, "Name"),
                            (1, 2, "Score"),
                            (2, 1, "Ana"),
                            (2, 2, "7"),
                            (3, 1, "Bo"),
                            (3, 2, ""),
                            (4, 1, "Cy"),
                            (4, 2, "12"),
                            (5, 1, "Di"),
                            (5, 2, "3"),
                        ],
                    ),
                    tab("Notes", &[(1, 1, "=Team!B2"), (2, 1, "=SUM(Team!B2:B5)")]),
                ],
                &[Operation::Sort {
                    tab: 0,
                    rows: (2, 5),
                    columns: (1, 2),
                    by: vec![(2, true)],
                }],
            )
            .expect("sorted");
        let names = (2..=5)
            .map(|row| cell(&output, 0, row, 1).expect("name").input.clone())
            .collect::<Vec<_>>();
        // Highest first, the empty score last.
        assert_eq!(names, ["Cy", "Ana", "Di", "Bo"]);
        // Ana's score moved from B2 to B3; the note follows it.
        let note = cell(&output, 1, 1, 1).expect("note");
        assert_eq!(note.input, "=Team!B3");
        assert_eq!(note.value, Value::Number(7.0));
        assert_eq!(
            cell(&output, 1, 2, 1).expect("sum").value,
            Value::Number(22.0)
        );
        // Only the block moves: a note beside it stays where it is.
        let output = IronCalc
            .apply(
                &[tab(
                    "T",
                    &[(1, 1, "2"), (2, 1, "1"), (2, 2, "=A2*10"), (2, 3, "note")],
                )],
                &[Operation::Sort {
                    tab: 0,
                    rows: (1, 2),
                    columns: (1, 2),
                    by: vec![(1, false)],
                }],
            )
            .expect("sorted");
        assert_eq!(cell(&output, 0, 1, 1).expect("A1").input, "1");
        assert_eq!(cell(&output, 0, 1, 2).expect("B1").input, "=A1*10");
        assert_eq!(cell(&output, 0, 2, 1).expect("A2").input, "2");
        assert_eq!(cell(&output, 0, 2, 3).expect("C2").input, "note");
    }
}

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

use ironcalc_base::{Model, cell::CellValue, types::CellType};

/// One tab's cells as they're stored: (row, column) from 1, and the input.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabInput {
    pub name: String,
    pub cells: Vec<(i32, i32, String)>,
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
    pub input: String,
    pub value: Value,
}

/// One tab after the operations, its cells in row then column order.
#[derive(Clone, Debug, PartialEq)]
pub struct TabOutput {
    pub name: String,
    pub cells: Vec<CellOutput>,
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
        } => model.set_user_input(index(*tab, count)?, *row, *column, input.clone()),
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
        for (row, column, input) in &tab.cells {
            model
                .set_user_input(tab_index, *row, *column, input.clone())
                .map_err(whole)?;
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
        for cell in model.get_all_cells() {
            let input = model
                .get_localized_cell_content(cell.index, cell.row, cell.column)
                .map_err(whole)?;
            if input.is_empty() {
                continue;
            }
            let value = value_of(&model, cell.index, cell.row, cell.column).map_err(whole)?;
            cells.entry(cell.index).or_default().push(CellOutput {
                row: cell.row,
                column: cell.column,
                input,
                value,
            });
        }
        Ok(names
            .into_iter()
            .enumerate()
            .map(|(position, name)| {
                let mut tab_cells = cells.remove(&(position as u32)).unwrap_or_default();
                tab_cells.sort_by_key(|cell| (cell.row, cell.column));
                TabOutput {
                    name,
                    cells: tab_cells,
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
            for index in 0..original.len() {
                model
                    .rename_sheet_by_index(index as u32, &format!("aw import {index}"))
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
                cells: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut scratch = Model::new_empty("scratch", LOCALE, TIMEZONE, LANGUAGE).map_err(whole)?;
        for cell in model.get_all_cells() {
            let mut input = model
                .get_localized_cell_content(cell.index, cell.row, cell.column)
                .map_err(whole)?;
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
}

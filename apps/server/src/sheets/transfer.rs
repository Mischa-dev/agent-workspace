//! Sheets in and out as files (AWD-54 slice 3): Excel (.xlsx) both ways, and
//! CSV. Google Sheets goes through .xlsx.
//!
//! An import becomes a new sheet: the file's tabs and the inputs typed in
//! them (formulas stay formulas), held to the same limits as any sheet. An
//! .xlsx file is a zip, so it is unpacked once into nothing first, counting,
//! to refuse one that would unpack into far more than it weighs.
//!
//! Exports read the stored cells: .xlsx carries every tab with its formulas,
//! CSV one tab's values as they show.

use std::{
    collections::HashSet,
    io::{self, Cursor, Read},
};

use super::{
    Change, MAX_CELLS, MAX_COLUMNS, MAX_INPUT_CHARACTERS, MAX_ROWS, MAX_TABS, StoredCell,
    engine::{FormulaEngine, TabInput},
    normalize_tab_name, text_of,
};

/// The largest file an import takes.
pub const MAX_IMPORT_BYTES: usize = 5 * 1024 * 1024;
/// How far an .xlsx file may unpack.
const MAX_UNPACKED_BYTES: u64 = 100 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Xlsx,
    Csv,
}

impl Format {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "xlsx" => Some(Self::Xlsx),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::Csv => "text/csv; charset=utf-8",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Xlsx => "xlsx",
            Self::Csv => "csv",
        }
    }
}

/// The tabs of an imported file, checked against a sheet's limits.
pub fn read(
    engine: &impl FormulaEngine,
    format: Format,
    bytes: &[u8],
    tab_name: &str,
) -> Result<Vec<TabInput>, String> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(too_large());
    }
    let tabs = match format {
        Format::Xlsx => {
            unpacks_safely(bytes)?;
            engine
                .read_xlsx(bytes, &tab_names)
                .map_err(|error| error.message)?
        }
        Format::Csv => {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| "Save the CSV file as UTF-8 text and try again.".to_owned())?;
            vec![read_csv(text, tab_name)?]
        }
    };
    checked(tabs)
}

pub fn too_large() -> String {
    format!(
        "Import a file up to {} MB.",
        MAX_IMPORT_BYTES / (1024 * 1024)
    )
}

/// Refuses a zip that isn't one, has too many parts, or unpacks too far.
fn unpacks_safely(bytes: &[u8]) -> Result<(), String> {
    let unreadable = || "That isn't an Excel file (.xlsx) this can read.".to_owned();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| unreadable())?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(unreadable());
    }
    let mut left = MAX_UNPACKED_BYTES;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|_| unreadable())?;
        let read =
            io::copy(&mut entry.take(left + 1), &mut io::sink()).map_err(|_| unreadable())?;
        if read > left {
            return Err("That file unpacks into more than a sheet can hold.".to_owned());
        }
        left -= read;
    }
    Ok(())
}

/// One tab from CSV text (or tab-separated text), each field as typed.
fn read_csv(text: &str, tab_name: &str) -> Result<TabInput, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first_line = text.lines().next().unwrap_or("");
    let delimiter = if first_line.contains('\t') && !first_line.contains(',') {
        b'\t'
    } else {
        b','
    };
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes());
    let mut cells = Vec::new();
    for (row_index, record) in reader.records().enumerate() {
        let record = record.map_err(|error| match error.position() {
            Some(position) => format!("Line {} of the CSV can't be read.", position.line()),
            None => "The CSV can't be read.".to_owned(),
        })?;
        let row = i32::try_from(row_index + 1).map_err(|_| rows_message())?;
        if row > MAX_ROWS {
            return Err(rows_message());
        }
        for (column_index, field) in record.iter().enumerate() {
            if field.trim().is_empty() {
                continue;
            }
            let column = i32::try_from(column_index + 1).map_err(|_| columns_message())?;
            cells.push((row, column, field.to_owned()));
            if cells.len() > MAX_CELLS {
                return Err(cells_message());
            }
        }
    }
    Ok(TabInput {
        name: tab_name.to_owned(),
        cells,
    })
}

fn rows_message() -> String {
    format!("A tab holds up to {MAX_ROWS} rows.")
}

fn columns_message() -> String {
    format!("A tab holds up to {MAX_COLUMNS} columns (A to GR).")
}

fn cells_message() -> String {
    format!("A sheet holds up to {MAX_CELLS} filled cells.")
}

/// The names tabs take in a sheet: tidied, with a stand-in for one that
/// can't be used, and none twice. Tidy names come back as they were.
fn tab_names(raw: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    raw.iter()
        .enumerate()
        .map(|(index, raw)| {
            let mut name = normalize_tab_name(raw).unwrap_or_else(|| format!("Sheet{}", index + 1));
            let mut suffix = 2;
            while !seen.insert(name.to_lowercase()) {
                name = format!("{} {suffix}", name.chars().take(28).collect::<String>());
                suffix += 1;
            }
            name
        })
        .collect()
}

/// The tabs, with names a sheet takes, inside every limit.
fn checked(mut tabs: Vec<TabInput>) -> Result<Vec<TabInput>, String> {
    if tabs.is_empty() {
        return Err("That file has no tabs.".to_owned());
    }
    if tabs.len() > MAX_TABS {
        return Err(format!("A sheet holds up to {MAX_TABS} tabs."));
    }
    let names = tab_names(&tabs.iter().map(|tab| tab.name.clone()).collect::<Vec<_>>());
    let mut total = 0;
    for (tab, name) in tabs.iter_mut().zip(names) {
        tab.name = name;
        for (row, column, input) in &tab.cells {
            if *row < 1 || *row > MAX_ROWS {
                return Err(rows_message());
            }
            if *column < 1 || *column > MAX_COLUMNS {
                return Err(columns_message());
            }
            if input.chars().count() > MAX_INPUT_CHARACTERS {
                return Err(format!(
                    "A cell holds up to {MAX_INPUT_CHARACTERS} characters."
                ));
            }
        }
        total += tab.cells.len();
    }
    if total > MAX_CELLS {
        return Err(cells_message());
    }
    Ok(tabs)
}

/// The changes that fill a new sheet (whose one tab is `first_tab`) with
/// `tabs`.
pub fn changes_for(first_tab: &str, tabs: &[TabInput]) -> Vec<Change> {
    let mut changes =
        Vec::with_capacity(tabs.len() + tabs.iter().map(|tab| tab.cells.len()).sum::<usize>());
    for (index, tab) in tabs.iter().enumerate() {
        if index == 0 {
            if tab.name != first_tab {
                changes.push(Change::RenameTab {
                    tab: Some(first_tab.to_owned()),
                    name: tab.name.clone(),
                });
            }
        } else {
            changes.push(Change::AddTab {
                name: tab.name.clone(),
            });
        }
    }
    for tab in tabs {
        for (row, column, input) in &tab.cells {
            changes.push(Change::Cell {
                tab: Some(tab.name.clone()),
                row: *row,
                column: *column,
                input: input.clone(),
                if_revision: None,
            });
        }
    }
    changes
}

/// A tab's stored cells as inputs, for an .xlsx export.
pub fn input_of(name: &str, cells: &[&StoredCell]) -> TabInput {
    TabInput {
        name: name.to_owned(),
        cells: cells
            .iter()
            .map(|cell| (cell.row_number, cell.column_number, cell.input.clone()))
            .collect(),
    }
}

/// One tab's values as CSV, from A1 to its last filled row and column.
pub fn csv_of(cells: &[&StoredCell]) -> Result<Vec<u8>, String> {
    let rows = cells.iter().map(|cell| cell.row_number).max().unwrap_or(0);
    let columns = cells
        .iter()
        .map(|cell| cell.column_number)
        .max()
        .unwrap_or(0);
    let mut grid = vec![vec![String::new(); columns as usize]; rows as usize];
    for cell in cells {
        grid[(cell.row_number - 1) as usize][(cell.column_number - 1) as usize] =
            text_of(&cell.value());
    }
    let mut writer = csv::WriterBuilder::new()
        .flexible(true)
        .from_writer(Vec::new());
    for row in grid {
        writer
            .write_record(&row)
            .map_err(|error| error.to_string())?;
    }
    writer.into_inner().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheets::engine::{FormulaEngine, IronCalc, Value};

    #[test]
    fn renamed_tabs_keep_their_formulas() {
        let bytes = IronCalc
            .to_xlsx(&[
                TabInput {
                    name: "Q1  Sales".to_owned(),
                    cells: vec![(1, 1, "5".to_owned())],
                },
                TabInput {
                    name: "Total".to_owned(),
                    cells: vec![(1, 1, "='Q1  Sales'!A1*2".to_owned())],
                },
            ])
            .expect("written");
        let tabs = read(&IronCalc, Format::Xlsx, &bytes, "Import").expect("read");
        assert_eq!(tabs[0].name, "Q1 Sales");
        assert!(
            tabs[1].cells[0].2.contains("Q1 Sales"),
            "{:?}",
            tabs[1].cells
        );
        assert_eq!(
            IronCalc.apply(&tabs, &[]).expect("computed")[1].cells[0].value,
            Value::Number(10.0)
        );
    }

    #[test]
    fn csv_reads_as_typed_and_tabs_get_names_a_sheet_takes() {
        let tabs = read(
            &IronCalc,
            Format::Csv,
            "\u{feff}Item,Cost\nRent,1200\n,\"=SUM(B2:B2)\"\n".as_bytes(),
            "Costs",
        )
        .expect("read");
        assert_eq!(tabs[0].name, "Costs");
        assert_eq!(
            tabs[0].cells,
            vec![
                (1, 1, "Item".to_owned()),
                (1, 2, "Cost".to_owned()),
                (2, 1, "Rent".to_owned()),
                (2, 2, "1200".to_owned()),
                (3, 2, "=SUM(B2:B2)".to_owned()),
            ]
        );
        let tsv = read(&IronCalc, Format::Csv, b"a\tb\n1\t2", "T").expect("read");
        assert_eq!(tsv[0].cells.len(), 4);
        assert!(read(&IronCalc, Format::Csv, &[0xff, 0xfe], "T").is_err());

        let named = checked(vec![
            TabInput {
                name: "Plan".to_owned(),
                cells: vec![],
            },
            TabInput {
                name: "plan".to_owned(),
                cells: vec![],
            },
            TabInput {
                name: "   ".to_owned(),
                cells: vec![],
            },
        ])
        .expect("named");
        assert_eq!(
            named
                .iter()
                .map(|tab| tab.name.as_str())
                .collect::<Vec<_>>(),
            ["Plan", "plan 2", "Sheet3"]
        );
        assert!(
            checked(vec![TabInput {
                name: "T".to_owned(),
                cells: vec![(1, 201, "x".to_owned())]
            }])
            .is_err()
        );
    }

    #[test]
    fn a_zip_that_unpacks_too_far_is_refused() {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true);
        writer.start_file("xl/big.xml", options).expect("start");
        let block = vec![0_u8; 1024 * 1024];
        for _ in 0..101 {
            writer.write_all(&block).expect("write");
        }
        let bytes = writer.finish().expect("finish").into_inner();
        assert!(bytes.len() < MAX_IMPORT_BYTES);
        assert_eq!(
            read(&IronCalc, Format::Xlsx, &bytes, "T"),
            Err("That file unpacks into more than a sheet can hold.".to_owned())
        );
    }

    #[test]
    fn exports_carry_values_and_formulas() {
        let cell = |row, column, input: &str, kind: &str, value: serde_json::Value| StoredCell {
            tab_id: uuid::Uuid::nil(),
            row_number: row,
            column_number: column,
            input: input.to_owned(),
            kind: kind.to_owned(),
            value,
            revision: 1,
        };
        let cells = [
            cell(1, 1, "Name", "text", serde_json::json!("Name")),
            cell(2, 2, "=1/4", "number", serde_json::json!(0.25)),
            cell(
                3,
                1,
                "say, \"hi\"",
                "text",
                serde_json::json!("say, \"hi\""),
            ),
        ];
        let refs = cells.iter().collect::<Vec<_>>();
        assert_eq!(
            String::from_utf8(csv_of(&refs).expect("csv")).expect("utf8"),
            "Name,\n,0.25\n\"say, \"\"hi\"\"\",\n"
        );
        let xlsx = IronCalc.to_xlsx(&[input_of("Data", &refs)]).expect("xlsx");
        let back = read(&IronCalc, Format::Xlsx, &xlsx, "T").expect("read");
        assert!(back[0].cells.contains(&(2, 2, "=1/4".to_owned())));
    }
}

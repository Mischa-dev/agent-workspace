//! A sheet as a source (C1.20): named ranges, and reading a block of a
//! tab's values for whatever shows them elsewhere (a dashboard tile, a page
//! embed, an agent).
//!
//! A named range gives a block of one tab a name, such as "Budget" for
//! `Summary!A1:B12`. It moves with the rows and columns a write inserts or
//! deletes around it, and goes with its tab or when every cell of it is
//! deleted. A reference to a block is a named range's name, `A1:B12` (the
//! first tab), or `Tab!A1:B12` (`'Tab name'!A1:B12` when the name has
//! spaces).

use rmcp::schemars;
use serde::Serialize;
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::{
    MAX_COLUMNS, MAX_ROWS,
    address::{self, Range},
    engine::Value,
};
use crate::{discussion::anchors, error::AppError};

/// Named ranges one sheet keeps.
pub const MAX_NAMED_RANGES: i64 = 100;
/// The most cells one read returns.
pub const MAX_READ_CELLS: i64 = 10_000;
pub const MAX_NAME_CHARACTERS: usize = 60;

/// A named range as the web app and agents see it.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NamedRange {
    pub name: String,
    #[schemars(with = "String")]
    pub tab_id: Uuid,
    pub tab: String,
    /// The block, such as "A1:B12".
    pub range: String,
}

#[derive(FromRow)]
struct Row {
    name: String,
    tab_id: Uuid,
    tab: String,
    first_row: i32,
    last_row: i32,
    first_column: i32,
    last_column: i32,
}

impl Row {
    fn block(&self) -> Range {
        Range {
            rows: (self.first_row, self.last_row),
            columns: (self.first_column, self.last_column),
        }
    }
}

const SELECT: &str = "SELECT named.name, named.tab_id, tab.name AS tab, named.first_row, \
     named.last_row, named.first_column, named.last_column \
     FROM sheet_named_ranges named JOIN sheet_tabs tab ON tab.id = named.tab_id";

/// A sheet's named ranges, by name.
pub async fn named(
    executor: impl sqlx::PgExecutor<'_>,
    sheet_id: Uuid,
) -> Result<Vec<NamedRange>, AppError> {
    Ok(sqlx::query_as::<_, Row>(&format!(
        "{SELECT} WHERE named.sheet_id = $1 ORDER BY lower(named.name), named.id"
    ))
    .bind(sheet_id)
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|row| NamedRange {
        range: row.block().name(),
        name: row.name,
        tab_id: row.tab_id,
        tab: row.tab,
    })
    .collect())
}

/// A name a range can have: a letter or `_` first, then letters, digits,
/// `_` and `.`, up to 60 characters, and not a cell's name (such as `Q1`),
/// so a reference reads the same either way.
pub fn name_of(value: &str) -> Option<String> {
    let name = value.trim();
    let mut characters = name.chars();
    let first = characters.next()?;
    let fits = (first.is_alphabetic() || first == '_')
        && characters
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '.'))
        && name.chars().count() <= MAX_NAME_CHARACTERS
        && address::cell(name).is_none();
    fits.then(|| name.to_owned())
}

/// A block of one tab that a reference names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    pub tab_id: Uuid,
    pub tab: String,
    pub range: Range,
    /// The named range it came from, if one.
    pub name: Option<String>,
}

/// Splits `Tab!A1:B2` (or `'Tab name'!A1:B2`) into the tab and the block.
fn split_tab(reference: &str) -> (Option<String>, &str) {
    match reference.rsplit_once('!') {
        Some((tab, block)) => {
            let tab = tab.trim();
            let tab = tab
                .strip_prefix('\'')
                .and_then(|tab| tab.strip_suffix('\''))
                .map(|tab| tab.replace("''", "'"))
                .unwrap_or_else(|| tab.to_owned());
            (Some(tab), block)
        }
        None => (None, reference),
    }
}

/// A block of the sheet by its A1 name, on `tab` (by id or name) or the
/// tab the reference names, else the first. Named ranges aren't read.
pub async fn locate_block(
    connection: &mut PgConnection,
    sheet_id: Uuid,
    tab: Option<&str>,
    reference: &str,
) -> Result<Option<Located>, AppError> {
    let (named_tab, block) = split_tab(reference.trim());
    let Some(range) = address::range(block, MAX_ROWS, MAX_COLUMNS)
        .filter(|range| range.rows.1 <= MAX_ROWS && range.columns.1 <= MAX_COLUMNS)
    else {
        return Ok(None);
    };
    let wanted = named_tab
        .as_deref()
        .or(tab)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let tabs = super::tabs(&mut *connection, sheet_id).await?;
    let found = match wanted {
        None => tabs.into_iter().next(),
        Some(wanted) => {
            let id = wanted.parse::<Uuid>().ok();
            tabs.into_iter()
                .find(|tab| Some(tab.id) == id || tab.name.eq_ignore_ascii_case(wanted))
        }
    };
    Ok(found.map(|tab| Located {
        tab_id: tab.id,
        tab: tab.name,
        range,
        name: None,
    }))
}

/// What a reference names: a named range of the sheet (ignoring case), or
/// a block as [`locate_block`] reads it.
pub async fn resolve(
    connection: &mut PgConnection,
    sheet_id: Uuid,
    reference: &str,
) -> Result<Option<Located>, AppError> {
    let reference = reference.trim();
    if let Some(name) = name_of(reference) {
        let row = sqlx::query_as::<_, Row>(&format!(
            "{SELECT} WHERE named.sheet_id = $1 AND lower(named.name) = lower($2)"
        ))
        .bind(sheet_id)
        .bind(&name)
        .fetch_optional(&mut *connection)
        .await?;
        if let Some(row) = row {
            return Ok(Some(Located {
                range: row.block(),
                tab_id: row.tab_id,
                tab: row.tab,
                name: Some(row.name),
            }));
        }
    }
    locate_block(connection, sheet_id, None, reference).await
}

/// A block's values, row by row. Rows and columns past the last one the
/// tab uses are left out, and so are rows past [`MAX_READ_CELLS`] cells
/// (`truncated` says so).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub rows: Vec<Vec<Value>>,
    pub truncated: bool,
}

pub async fn read_range(
    connection: &mut PgConnection,
    tab_id: Uuid,
    range: Range,
) -> Result<Block, AppError> {
    let (used_rows, used_columns) = super::used_extent(&mut *connection, tab_id).await?;
    let rows = (range.rows.0, range.rows.1.min(used_rows));
    let columns = (range.columns.0, range.columns.1.min(used_columns));
    if rows.1 < rows.0 || columns.1 < columns.0 {
        return Ok(Block::default());
    }
    let width = i64::from(columns.1 - columns.0 + 1);
    let fit = i32::try_from((MAX_READ_CELLS / width).max(1)).unwrap_or(i32::MAX);
    let last = rows.1.min(rows.0.saturating_add(fit - 1));
    let cells = super::cells_in(
        &mut *connection,
        tab_id,
        (rows.0, last),
        columns,
        MAX_READ_CELLS,
        true,
    )
    .await?;
    let mut block = vec![
        vec![Value::Empty; usize::try_from(width).unwrap_or_default()];
        usize::try_from(last - rows.0 + 1).unwrap_or_default()
    ];
    for cell in cells {
        let row = usize::try_from(cell.row_number - rows.0).unwrap_or_default();
        let column = usize::try_from(cell.column_number - columns.0).unwrap_or_default();
        if let Some(slot) = block.get_mut(row).and_then(|row| row.get_mut(column)) {
            *slot = cell.value();
        }
    }
    Ok(Block {
        rows: block,
        truncated: last < rows.1,
    })
}

/// Moves a tab's named ranges with the rows and columns a write inserted
/// or deleted, in the write's transaction; a range whose cells were all
/// deleted goes.
pub async fn follow_moves(
    connection: &mut PgConnection,
    tab_id: Uuid,
    moves: &[anchors::GridMove],
) -> Result<(), AppError> {
    if moves.is_empty() {
        return Ok(());
    }
    let ranges = sqlx::query_as::<_, (Uuid, i32, i32, i32, i32)>(
        "SELECT id, first_row, last_row, first_column, last_column FROM sheet_named_ranges \
         WHERE tab_id = $1 ORDER BY id FOR UPDATE",
    )
    .bind(tab_id)
    .fetch_all(&mut *connection)
    .await?;
    for (id, first_row, last_row, first_column, last_column) in ranges {
        // Anchors count from 0.
        let anchor = serde_json::json!({
            "kind": "range",
            "from": [first_row - 1, first_column - 1],
            "to": [last_row - 1, last_column - 1],
        });
        let moved = anchors::follow(&anchor, moves);
        if moved.as_ref() == Some(&anchor) {
            continue;
        }
        let corner = |value: Option<&serde_json::Value>, index: usize| {
            value
                .and_then(|value| value.get(index))
                .and_then(serde_json::Value::as_i64)
                .and_then(|place| i32::try_from(place + 1).ok())
        };
        let from = moved.as_ref().and_then(|moved| moved.get("from"));
        let to = moved.as_ref().and_then(|moved| moved.get("to"));
        match (
            corner(from, 0),
            corner(to, 0),
            corner(from, 1),
            corner(to, 1),
        ) {
            (Some(first_row), Some(last_row), Some(first_column), Some(last_column)) => {
                sqlx::query(
                    "UPDATE sheet_named_ranges SET first_row = $2, last_row = $3, \
                       first_column = $4, last_column = $5, updated_at = now() WHERE id = $1",
                )
                .bind(id)
                .bind(first_row)
                .bind(last_row)
                .bind(first_column)
                .bind(last_column)
                .execute(&mut *connection)
                .await?;
            }
            _ => {
                sqlx::query("DELETE FROM sheet_named_ranges WHERE id = $1")
                    .bind(id)
                    .execute(&mut *connection)
                    .await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_not_cells() {
        assert_eq!(name_of(" Budget "), Some("Budget".to_owned()));
        assert_eq!(name_of("_q1.totals"), Some("_q1.totals".to_owned()));
        assert_eq!(name_of("Q1"), None);
        assert_eq!(name_of("1st"), None);
        assert_eq!(name_of("Two words"), None);
        assert_eq!(name_of("A1:B2"), None);
        assert_eq!(name_of(&"x".repeat(61)), None);
    }

    #[test]
    fn a_reference_can_name_its_tab() {
        assert_eq!(
            split_tab("Summary!A1:B2"),
            (Some("Summary".to_owned()), "A1:B2")
        );
        assert_eq!(split_tab("'Q1 ''24'!B3"), (Some("Q1 '24".to_owned()), "B3"));
        assert_eq!(split_tab("A1:B2"), (None, "A1:B2"));
    }
}

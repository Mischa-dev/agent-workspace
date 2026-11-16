//! Anchors: where on a thing a comment thread is about (C1.3).
//!
//! ```jsonc
//! {"kind": "text", "blockId": "b7Qx", "quote": "the launch date"}       // page
//! {"kind": "cell", "tabId": "…", "row": 12, "column": 3}                // sheet
//! {"kind": "range", "tabId": "…", "from": [2, 1], "to": [9, 4]}         // sheet
//! {"kind": "elements", "elementIds": ["sticky-1"], "at": {"x": 1, "y": 2}} // board
//! {"kind": "field", "fieldId": "…"} or {"kind": "field", "field": "description"} // record, task
//! ```
//!
//! `check` refuses a shape the thing doesn't take and returns the anchor
//! rebuilt from the fields it knows, so nothing else is stored. A sheet
//! anchor's tab must be one of the sheet's tabs. Rows and columns count from
//! zero. Anchors follow the thing: C1.19 moves sheet anchors with row and
//! column changes; a page's block ids last, and a board falls back to `at`.

use serde_json::{Map, Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::subjects::{AnchorShape, Subject};
use crate::error::AppError;

const MAX_QUOTE: usize = 500;
const MAX_ID: usize = 128;
const MAX_ELEMENTS: usize = 50;
const MAX_ROW: i64 = 1_000_000;
const MAX_COLUMN: i64 = 16_384;
const TASK_FIELDS: &[&str] = &["title", "description"];

fn refused(message: &'static str) -> AppError {
    AppError::Refused {
        field: "anchor",
        message,
    }
}

fn text(object: &Map<String, Value>, key: &str, max: usize) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().count() <= max)
        .filter(|value| !value.chars().any(char::is_control))
        .map(str::to_owned)
}

fn cell(value: Option<&Value>) -> Option<(i64, i64)> {
    let pair = value?.as_array()?;
    match pair.as_slice() {
        [row, column] => in_grid(row.as_i64()?, column.as_i64()?),
        _ => None,
    }
}

fn in_grid(row: i64, column: i64) -> Option<(i64, i64)> {
    ((0..MAX_ROW).contains(&row) && (0..MAX_COLUMN).contains(&column)).then_some((row, column))
}

/// The anchor as stored, or why it doesn't fit `subject`.
pub async fn check(
    connection: &mut PgConnection,
    subject: &Subject,
    thing_id: Uuid,
    anchor: &Value,
) -> Result<Value, AppError> {
    let object = anchor
        .as_object()
        .ok_or(refused("An anchor is an object with a kind."))?;
    let kind = object.get("kind").and_then(Value::as_str).unwrap_or("");
    match (subject.anchor, kind) {
        (AnchorShape::Text, "text") => {
            let block_id = text(object, "blockId", MAX_ID)
                .ok_or(refused("A page anchor names its blockId."))?;
            let quote = text(object, "quote", MAX_QUOTE)
                .ok_or(refused("A page anchor quotes up to 500 characters."))?;
            Ok(json!({ "kind": "text", "blockId": block_id, "quote": quote }))
        }
        (AnchorShape::Cells, "cell" | "range") => {
            let tab_id = object
                .get("tabId")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
                .ok_or(refused("A sheet anchor names its tabId."))?;
            let on_sheet = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM sheet_tabs WHERE id = $1 AND sheet_id = $2)",
            )
            .bind(tab_id)
            .bind(thing_id)
            .fetch_one(&mut *connection)
            .await?;
            if !on_sheet {
                return Err(refused("That tab isn't in this sheet."));
            }
            if kind == "cell" {
                let (row, column) = object
                    .get("row")
                    .and_then(Value::as_i64)
                    .zip(object.get("column").and_then(Value::as_i64))
                    .and_then(|(row, column)| in_grid(row, column))
                    .ok_or(refused("A cell anchor has a row and a column from 0."))?;
                Ok(json!({ "kind": "cell", "tabId": tab_id, "row": row, "column": column }))
            } else {
                let (from, to) =
                    cell(object.get("from"))
                        .zip(cell(object.get("to")))
                        .ok_or(refused(
                            "A range anchor has from and to, each [row, column].",
                        ))?;
                let top_left = [from.0.min(to.0), from.1.min(to.1)];
                let bottom_right = [from.0.max(to.0), from.1.max(to.1)];
                Ok(json!({
                    "kind": "range", "tabId": tab_id, "from": top_left, "to": bottom_right,
                }))
            }
        }
        (AnchorShape::Elements, "elements") => {
            let ids = object
                .get("elementIds")
                .and_then(Value::as_array)
                .filter(|ids| !ids.is_empty() && ids.len() <= MAX_ELEMENTS)
                .ok_or(refused("A board anchor names 1 to 50 elementIds."))?
                .iter()
                .map(|id| {
                    id.as_str()
                        .filter(|id| !id.is_empty() && id.len() <= MAX_ID)
                        .filter(|id| !id.chars().any(char::is_control))
                        .map(str::to_owned)
                        .ok_or(refused("Each element id is a short string."))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let at = object
                .get("at")
                .and_then(Value::as_object)
                .and_then(|at| {
                    let x = at.get("x")?.as_f64()?;
                    let y = at.get("y")?.as_f64()?;
                    (x.is_finite() && y.is_finite()).then_some((x, y))
                })
                .ok_or(refused("A board anchor has a point, at: {x, y}."))?;
            Ok(json!({ "kind": "elements", "elementIds": ids, "at": { "x": at.0, "y": at.1 } }))
        }
        (AnchorShape::Field, "field") => {
            if let Some(field_id) = object
                .get("fieldId")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
            {
                return Ok(json!({ "kind": "field", "fieldId": field_id }));
            }
            let field = object
                .get("field")
                .and_then(Value::as_str)
                .filter(|field| TASK_FIELDS.contains(field))
                .ok_or(refused(
                    "A field anchor names a fieldId, or field: title or description.",
                ))?;
            Ok(json!({ "kind": "field", "field": field }))
        }
        _ => Err(refused("That kind of anchor doesn't fit this thing.")),
    }
}

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

/// A page anchor's quote, as page comments always allowed.
const MAX_TEXT_QUOTE: usize = 1_000;
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
            // A quote is optional: without one the thread is about the
            // whole block.
            match object.get("quote") {
                None | Some(Value::Null) => Ok(json!({ "kind": "text", "blockId": block_id })),
                Some(_) => {
                    let quote = text(object, "quote", MAX_TEXT_QUOTE)
                        .ok_or(refused("A page anchor quotes up to 1,000 characters."))?;
                    Ok(json!({ "kind": "text", "blockId": block_id, "quote": quote }))
                }
            }
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

/// A row or column insert or delete on a sheet tab, in the order a write
/// made them, each `at` counted in the grid as it stood then (C1.19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridMove {
    pub rows: bool,
    pub insert: bool,
    pub at: i64,
    pub count: i64,
}

/// A span `[start, end]` on one axis after a move on that axis, or `None`
/// when the move deleted all of it. An insert inside a span widens it; a
/// delete that takes part of it narrows it.
fn follow_span(start: i64, end: i64, step: &GridMove) -> Option<(i64, i64)> {
    let (at, count) = (step.at, step.count);
    if step.insert {
        let shift = |place: i64| if place >= at { place + count } else { place };
        return Some((shift(start), shift(end)));
    }
    let gone_end = at + count;
    let start = if start < at {
        start
    } else if start >= gone_end {
        start - count
    } else {
        at
    };
    let end = if end < at {
        end
    } else if end >= gone_end {
        end - count
    } else {
        at - 1
    };
    (end >= start).then_some((start, end))
}

/// A sheet anchor after `moves`: the same shape at its new place, or `None`
/// when its cells were deleted or pushed off the grid. Other anchors come
/// back as they are.
pub fn follow(anchor: &Value, moves: &[GridMove]) -> Option<Value> {
    let kind = anchor.get("kind").and_then(Value::as_str).unwrap_or("");
    let (mut rows, mut columns) = match kind {
        "cell" => {
            let row = anchor.get("row")?.as_i64()?;
            let column = anchor.get("column")?.as_i64()?;
            ((row, row), (column, column))
        }
        "range" => {
            let from = cell(anchor.get("from"))?;
            let to = cell(anchor.get("to"))?;
            ((from.0, to.0), (from.1, to.1))
        }
        _ => return Some(anchor.clone()),
    };
    for step in moves {
        let span = if step.rows { &mut rows } else { &mut columns };
        *span = follow_span(span.0, span.1, step)?;
    }
    in_grid(rows.1, columns.1)?;
    let mut moved = anchor.clone();
    if kind == "cell" {
        moved["row"] = json!(rows.0);
        moved["column"] = json!(columns.0);
    } else {
        moved["from"] = json!([rows.0, columns.0]);
        moved["to"] = json!([rows.1, columns.1]);
    }
    Some(moved)
}

/// Moves the anchors of a sheet tab's comment threads with the rows and
/// columns a write inserted or deleted (C1.19), in the write's transaction.
/// A thread whose cells were deleted keeps its comments and loses its
/// anchor, so it stays about the sheet as a whole.
pub async fn follow_sheet_moves(
    connection: &mut PgConnection,
    sheet_id: Uuid,
    tab_id: Uuid,
    moves: &[GridMove],
) -> Result<(), AppError> {
    if moves.is_empty() {
        return Ok(());
    }
    let threads = sqlx::query_as::<_, (Uuid, Value)>(
        r#"
        -- visibility: the sheet's own write moving its threads' anchors; nothing is read out
        SELECT thread.id, thread.anchor
        FROM conversations thread
        JOIN conversations comments ON comments.id = thread.parent_channel_id
        WHERE comments.kind = 'comments'
          AND comments.subject_kind = 'sheet'
          AND comments.subject_id = $1
          AND thread.anchor ->> 'tabId' = $2::text
        ORDER BY thread.id
        FOR UPDATE OF thread
        "#,
    )
    .bind(sheet_id)
    .bind(tab_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut ids = Vec::new();
    let mut anchors = Vec::new();
    for (id, anchor) in threads {
        let moved = follow(&anchor, moves);
        if moved.as_ref() != Some(&anchor) {
            ids.push(id);
            anchors.push(moved);
        }
    }
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE conversations thread
        SET anchor = moved.anchor, updated_at = now()
        FROM UNNEST($1::uuid[], $2::jsonb[]) AS moved(id, anchor)
        WHERE thread.id = moved.id
        "#,
    )
    .bind(&ids)
    .bind(&anchors)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(insert: bool, at: i64, count: i64) -> GridMove {
        GridMove {
            rows: true,
            insert,
            at,
            count,
        }
    }

    #[test]
    fn cell_anchors_follow_rows_and_columns() {
        let tab = "00000000-0000-0000-0000-000000000001";
        let anchor = json!({"kind": "cell", "tabId": tab, "row": 4, "column": 2});
        let moved = follow(&anchor, &[rows(true, 4, 2)]).expect("kept");
        assert_eq!(moved["row"], 6);
        let moved = follow(&anchor, &[rows(true, 5, 2)]).expect("kept");
        assert_eq!(moved["row"], 4);
        let moved = follow(
            &anchor,
            &[GridMove {
                rows: false,
                insert: false,
                at: 0,
                count: 2,
            }],
        )
        .expect("kept");
        assert_eq!(moved["column"], 0);
        assert_eq!(moved["tabId"], tab);
        assert!(follow(&anchor, &[rows(false, 3, 2)]).is_none());
        assert_eq!(
            follow(&anchor, &[rows(false, 0, 2), rows(true, 0, 1)]).expect("kept")["row"],
            3
        );
    }

    #[test]
    fn range_anchors_grow_and_shrink() {
        let anchor = json!({"kind": "range", "tabId": "t", "from": [2, 1], "to": [6, 3]});
        let grown = follow(&anchor, &[rows(true, 4, 3)]).expect("kept");
        assert_eq!(grown["from"], json!([2, 1]));
        assert_eq!(grown["to"], json!([9, 3]));
        let cut = follow(&anchor, &[rows(false, 1, 3)]).expect("kept");
        assert_eq!(cut["from"], json!([1, 1]));
        assert_eq!(cut["to"], json!([3, 3]));
        assert!(follow(&anchor, &[rows(false, 0, 10)]).is_none());
        let page = json!({"kind": "text", "blockId": "b", "quote": "q"});
        assert_eq!(follow(&page, &[rows(false, 0, 10)]), Some(page));
    }
}

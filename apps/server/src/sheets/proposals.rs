//! Proposals on sheets (C1.11): someone whose role only suggests on sheets
//! writes cells as usual, and the write is kept as a proposal instead.
//!
//! A proposal is the cells it would set, each with the input it read
//! (`before`) and the one it proposes (`after`), worked out by running the
//! write inside a savepoint and rolling it back, so a row by key or a
//! formula lands exactly where the real write would put it. Accepting
//! applies the chosen cells as the author's write, and is refused when any
//! of them changed since, so it never undoes a change the author hadn't
//! seen. Rows and columns inserted or deleted later move a waiting
//! proposal's cells with them (a deleted cell can't be accepted). Only cell
//! contents can be proposed; tabs, rows and looks need edit access.

use serde::{Deserialize, Serialize};
use sqlx::{Connection, Postgres, Transaction};
use uuid::Uuid;

use super::{Change, Problem, Sheet};
use crate::{
    error::AppError,
    events::{self, NewEvent, catalog},
    memory::Writer,
    page_docs::suggestions::{Decided, Decision, Refusal},
};

/// The most cells one proposal sets.
pub const MAX_PROPOSED_CELLS: usize = 500;
/// The most proposals one author keeps waiting on one sheet.
const MAX_PENDING_PER_AUTHOR: i64 = 20;

/// One cell a proposal sets.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProposedCell {
    pub tab_id: Uuid,
    pub tab: String,
    pub row: i32,
    pub column: i32,
    /// The input it read; empty for an empty cell.
    pub before: String,
    /// The input it proposes; empty clears the cell.
    pub after: String,
    /// Its row or column was deleted since it was proposed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gone: bool,
}

impl ProposedCell {
    pub fn address(&self) -> String {
        format!("{}{}", super::column_name(self.column), self.row)
    }
}

/// What proposing came to.
#[derive(Debug)]
pub enum Proposed {
    Made {
        id: Uuid,
        cells: Vec<ProposedCell>,
    },
    /// The write wouldn't change any cell.
    Unchanged,
    /// The write itself is wrong, as the direct write would have said.
    Problem(Problem),
    /// Something other than cell contents, or too much at once.
    Refused(&'static str),
}

/// Whether every change only sets cell contents.
pub fn only_cells(changes: &[Change]) -> bool {
    changes
        .iter()
        .all(|change| matches!(change, Change::Cell { .. } | Change::Row { .. }))
}

/// Keeps `changes` to `sheet` as a proposal by `author`. The caller has
/// checked the author may suggest on the sheet; the sheet is locked here.
pub async fn propose(
    transaction: &mut Transaction<'_, Postgres>,
    author: Writer,
    sheet: &Sheet,
    changes: &[Change],
    note: Option<&str>,
) -> Result<Proposed, AppError> {
    if !only_cells(changes) {
        return Ok(Proposed::Refused(
            "Your role here suggests changes to sheets: you can propose cell contents, but adding \
             or removing tabs, rows and columns, and formatting, need edit access.",
        ));
    }
    let Some(sheet) = super::lock(transaction, sheet.project_id, sheet.id).await? else {
        return Ok(Proposed::Refused("That sheet is gone."));
    };
    if sheet.archived {
        return Ok(Proposed::Refused(
            "That sheet is archived. Someone who edits it can restore it.",
        ));
    }
    // Run the write for real, read what it set, and roll it back.
    let set = {
        let mut trial = (**transaction).begin().await?;
        let outcome = super::apply(
            &mut trial,
            &sheet,
            author.actor_id,
            changes,
            MAX_PROPOSED_CELLS + 1,
        )
        .await?;
        trial.rollback().await?;
        match outcome {
            Err(problem) => return Ok(Proposed::Problem(problem)),
            Ok(applied) if applied.inputs_changed > MAX_PROPOSED_CELLS => {
                return Ok(Proposed::Refused("Propose up to 500 cells at a time."));
            }
            Ok(applied) => applied
                .changed
                .into_iter()
                .take(applied.inputs_changed)
                .collect::<Vec<_>>(),
        }
    };
    let mut cells = Vec::with_capacity(set.len());
    for cell in set {
        let Some((tab_id, before)) = sqlx::query_as::<_, (Uuid, Option<String>)>(
            "SELECT tab.id, cell.input FROM sheet_tabs tab \
             LEFT JOIN sheet_cells cell ON cell.tab_id = tab.id \
                  AND cell.row_number = $3 AND cell.column_number = $4 \
             WHERE tab.sheet_id = $1 AND tab.name = $2",
        )
        .bind(sheet.id)
        .bind(&cell.tab)
        .bind(cell.row)
        .bind(cell.column)
        .fetch_optional(&mut **transaction)
        .await?
        else {
            return Ok(Proposed::Refused(
                "Proposals set cells on the sheet's tabs as they are.",
            ));
        };
        let before = before.unwrap_or_default();
        let after = cell.input.unwrap_or_default();
        if before != after {
            cells.push(ProposedCell {
                tab_id,
                tab: cell.tab,
                row: cell.row,
                column: cell.column,
                before,
                after,
                gone: false,
            });
        }
    }
    if cells.is_empty() {
        return Ok(Proposed::Unchanged);
    }
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM proposals \
         WHERE thing_kind = 'sheet' AND thing_id = $1 AND author_actor_id = $2 \
           AND status = 'pending'",
    )
    .bind(sheet.id)
    .bind(author.actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if pending >= MAX_PENDING_PER_AUTHOR {
        return Ok(Proposed::Refused(
            "You have too many proposals waiting on this sheet. Wait for some to be decided, or \
             withdraw them.",
        ));
    }
    let id = Uuid::now_v7();
    let workspace_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO proposals (
            id, workspace_id, space_id, thing_kind, thing_id, operation, author_actor_id,
            via_oauth_grant_id, note, base_version, change
        )
        SELECT $1, project.organization_id, project.id, 'sheet', $3, 'sheet_write', $4, $5, $6,
               $7, jsonb_build_object('cells', $8::jsonb)
        FROM projects project WHERE project.id = $2
        RETURNING workspace_id
        "#,
    )
    .bind(id)
    .bind(sheet.project_id)
    .bind(sheet.id)
    .bind(author.actor_id)
    .bind(author.via_grant_id)
    .bind(note)
    .bind(sheet.revision.max(1))
    .bind(serde_json::to_value(&cells).map_err(|_| AppError::Internal("proposed cells"))?)
    .fetch_one(&mut **transaction)
    .await?;
    announce(
        transaction,
        workspace_id,
        sheet.project_id,
        sheet.id,
        author.actor_id,
    )
    .await?;
    crate::proposals::announce_fact(transaction, id, false).await?;
    Ok(Proposed::Made { id, cells })
}

/// Accepts (all of it, or the cells at `parts`), rejects or withdraws a
/// proposal on `sheet_id`. `may_edit` is whether `decider` edits the sheet
/// directly; accepting and rejecting need it, and only the author
/// withdraws.
pub async fn decide(
    transaction: &mut Transaction<'_, Postgres>,
    decider: Writer,
    may_edit: bool,
    sheet_id: Uuid,
    proposal_id: Uuid,
    decision: Decision,
    parts: Option<&[u16]>,
) -> Result<Result<Decided, Refusal>, AppError> {
    let Some((space_id, workspace_id, author_id, status, sqlx::types::Json(cells))) =
        sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                Uuid,
                String,
                sqlx::types::Json<Vec<ProposedCell>>,
            ),
        >(
            "SELECT space_id, workspace_id, author_actor_id, status, change -> 'cells' \
             FROM proposals WHERE id = $1 AND thing_kind = 'sheet' AND thing_id = $2 \
             FOR UPDATE",
        )
        .bind(proposal_id)
        .bind(sheet_id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(Err(Refusal::NotFound));
    };
    if status != "pending" {
        return Ok(Err(Refusal::AlreadyDecided(status)));
    }
    match decision {
        Decision::Withdraw if decider.actor_id != author_id => {
            return Ok(Err(Refusal::NotYours(
                "Only its author can withdraw a proposal. Reject it instead.",
            )));
        }
        Decision::Accept | Decision::Reject if !may_edit => {
            return Ok(Err(Refusal::NotYours(
                "Deciding a proposal needs edit access to the sheet. Ask someone who edits it.",
            )));
        }
        Decision::Withdraw | Decision::Reject if parts.is_some() => {
            return Ok(Err(Refusal::NotYours(
                "Parts are for accepting; a rejection or withdrawal takes the whole proposal.",
            )));
        }
        _ => {}
    }
    let Some(sheet) = super::lock(transaction, space_id, sheet_id).await? else {
        return Ok(Err(Refusal::NotFound));
    };
    let mut version = sheet.revision;
    let mut applied_change_set = None;
    let status = match decision {
        Decision::Accept => {
            if sheet.archived {
                return Ok(Err(Refusal::Archived));
            }
            let chosen: Vec<&ProposedCell> = match parts {
                None => cells.iter().collect(),
                Some(parts) => {
                    let mut chosen = Vec::with_capacity(parts.len());
                    for part in parts {
                        match cells.get(usize::from(*part)) {
                            Some(cell) if !chosen.contains(&cell) => chosen.push(cell),
                            Some(_) => {}
                            None => {
                                return Ok(Err(Refusal::Stale(format!(
                                    "It has {} cells, numbered from 0; there's no part {part}.",
                                    cells.len()
                                ))));
                            }
                        }
                    }
                    chosen
                }
            };
            if chosen.is_empty() {
                return Ok(Err(Refusal::Stale(
                    "Choose at least one of its cells to accept.".to_owned(),
                )));
            }
            for cell in &chosen {
                if cell.gone {
                    return Ok(Err(Refusal::Stale(format!(
                        "{} on {} was deleted since it was proposed.",
                        cell.address(),
                        cell.tab
                    ))));
                }
                let now = sqlx::query_scalar::<_, Option<String>>(
                    "SELECT (SELECT input FROM sheet_cells \
                              WHERE tab_id = tab.id AND row_number = $3 AND column_number = $4) \
                     FROM sheet_tabs tab WHERE tab.id = $1 AND tab.sheet_id = $2",
                )
                .bind(cell.tab_id)
                .bind(sheet_id)
                .bind(cell.row)
                .bind(cell.column)
                .fetch_optional(&mut **transaction)
                .await?;
                let Some(now) = now else {
                    return Ok(Err(Refusal::Stale(format!(
                        "The tab \"{}\" is no longer on the sheet.",
                        cell.tab
                    ))));
                };
                if now.unwrap_or_default() != cell.before {
                    return Ok(Err(Refusal::Stale(format!(
                        "{} on {} has changed since it was proposed.",
                        cell.address(),
                        cell.tab
                    ))));
                }
            }
            let changes: Vec<Change> = chosen
                .iter()
                .map(|cell| Change::Cell {
                    tab: Some(cell.tab_id.to_string()),
                    row: cell.row,
                    column: cell.column,
                    input: cell.after.clone(),
                    if_revision: None,
                })
                .collect();
            // The cells are the author's: the history says so.
            let applied = match super::apply(transaction, &sheet, author_id, &changes, 0).await? {
                Ok(applied) => applied,
                Err(problem) => return Ok(Err(Refusal::Stale(problem.message))),
            };
            if applied.revision != sheet.revision {
                crate::outbox::enqueue_sheet_updated(
                    transaction,
                    space_id,
                    sheet_id,
                    applied.revision,
                )
                .await?;
            }
            version = applied.revision;
            applied_change_set =
                crate::change_sets::at_version(transaction, "sheet", sheet_id, version).await?;
            if chosen.len() == cells.len() {
                "accepted"
            } else {
                "partly_accepted"
            }
        }
        Decision::Reject => "rejected",
        Decision::Withdraw => "withdrawn",
    };
    let applied = matches!(status, "accepted" | "partly_accepted");
    sqlx::query(
        "UPDATE proposals SET status = $2, decided_by_actor_id = $3, decided_at = now(), \
                applied_version = $4, applied_change_set_id = $5, updated_at = now() \
         WHERE id = $1",
    )
    .bind(proposal_id)
    .bind(status)
    .bind(decider.actor_id)
    .bind(applied.then_some(version))
    .bind(applied_change_set)
    .execute(&mut **transaction)
    .await?;
    announce(
        transaction,
        workspace_id,
        space_id,
        sheet_id,
        decider.actor_id,
    )
    .await?;
    Ok(Ok(Decided { status, version }))
}

/// Moves the cells of `sheet_id`'s waiting proposals on `tab_id` with the
/// rows and columns a write inserted or deleted, in the write's
/// transaction, so accepting one later sets the cells the author meant. A
/// cell whose row or column went stays in its place in the list (parts are
/// numbered by it) and can't be accepted.
pub async fn follow_moves(
    connection: &mut sqlx::PgConnection,
    sheet_id: Uuid,
    tab_id: Uuid,
    moves: &[crate::discussion::anchors::GridMove],
) -> Result<(), AppError> {
    if moves.is_empty() {
        return Ok(());
    }
    let waiting = sqlx::query_as::<_, (Uuid, sqlx::types::Json<Vec<ProposedCell>>)>(
        "SELECT id, change -> 'cells' FROM proposals \
         WHERE thing_kind = 'sheet' AND thing_id = $1 AND status = 'pending' \
           AND change -> 'cells' @> jsonb_build_array(jsonb_build_object('tabId', $2::text)) \
         ORDER BY id FOR UPDATE",
    )
    .bind(sheet_id)
    .bind(tab_id)
    .fetch_all(&mut *connection)
    .await?;
    for (id, sqlx::types::Json(mut cells)) in waiting {
        if !move_cells(&mut cells, tab_id, moves) {
            continue;
        }
        sqlx::query(
            "UPDATE proposals SET change = jsonb_set(change, '{cells}', $2::jsonb), \
                    updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(serde_json::to_value(&cells).map_err(|_| AppError::Internal("proposed cells"))?)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

/// Moves the cells on `tab_id` (see [`follow_moves`]); whether any moved.
fn move_cells(
    cells: &mut [ProposedCell],
    tab_id: Uuid,
    moves: &[crate::discussion::anchors::GridMove],
) -> bool {
    let mut moved = false;
    // A proposed formula's references move too, so accepting it later reads
    // the cells it meant: on its own tab, and on others where it names the
    // moved tab.
    let tab_name = cells
        .iter()
        .find(|cell| cell.tab_id == tab_id)
        .map(|cell| cell.tab.clone());
    if let Some(tab_name) = &tab_name {
        for cell in cells.iter_mut().filter(|cell| !cell.gone) {
            if !cell.after.starts_with('=') {
                continue;
            }
            let shifted = shift_formula(&cell.after, tab_name, cell.tab_id == tab_id, moves);
            if shifted != cell.after {
                cell.after = shifted;
                moved = true;
            }
        }
    }
    for cell in cells
        .iter_mut()
        .filter(|cell| cell.tab_id == tab_id && !cell.gone)
    {
        // Anchors count from 0; cells from 1.
        let anchor = serde_json::json!({
            "kind": "cell",
            "row": i64::from(cell.row) - 1,
            "column": i64::from(cell.column) - 1,
        });
        let place = crate::discussion::anchors::follow(&anchor, moves).and_then(|anchor| {
            let row = i32::try_from(anchor.get("row")?.as_i64()? + 1).ok()?;
            let column = i32::try_from(anchor.get("column")?.as_i64()? + 1).ok()?;
            Some((row, column))
        });
        match place {
            Some((row, column)) if (row, column) == (cell.row, cell.column) => {}
            Some((row, column)) => {
                cell.row = row;
                cell.column = column;
                moved = true;
            }
            None => {
                cell.gone = true;
                moved = true;
            }
        }
    }
    moved
}

/// A cell reference in a formula: `$A$2` as its parts, and where it ends.
struct CellRef {
    column_fixed: bool,
    column: i64,
    row_fixed: bool,
    row: i64,
    end: usize,
}

/// Reads a cell reference (`A2`, `$B$10`) at `at`, if one is there.
fn cell_ref(chars: &[char], at: usize) -> Option<CellRef> {
    let mut index = at;
    let column_fixed = chars.get(index) == Some(&'$');
    if column_fixed {
        index += 1;
    }
    let letters = chars[index..]
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    if !(1..=3).contains(&letters) {
        return None;
    }
    let column = chars[index..index + letters].iter().fold(0_i64, |sum, c| {
        sum * 26 + i64::from(c.to_ascii_uppercase() as u8 - b'A' + 1)
    });
    index += letters;
    let row_fixed = chars.get(index) == Some(&'$');
    if row_fixed {
        index += 1;
    }
    let digits = chars[index..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count();
    if !(1..=7).contains(&digits) {
        return None;
    }
    let row = chars[index..index + digits]
        .iter()
        .collect::<String>()
        .parse::<i64>()
        .ok()?;
    index += digits;
    // A function name such as LOG10( or a longer word isn't a reference.
    if chars
        .get(index)
        .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '(' | '!'))
    {
        return None;
    }
    Some(CellRef {
        column_fixed,
        column,
        row_fixed,
        row,
        end: index,
    })
}

/// A sheet name before `!` at `at` (`Plan!` or `'Q3 plan'!`): the name and
/// where the reference after it starts.
fn sheet_prefix(chars: &[char], at: usize) -> Option<(String, usize)> {
    if chars.get(at) == Some(&'\'') {
        let mut name = String::new();
        let mut index = at + 1;
        loop {
            match chars.get(index)? {
                '\'' if chars.get(index + 1) == Some(&'\'') => {
                    name.push('\'');
                    index += 2;
                }
                '\'' => break,
                c => {
                    name.push(*c);
                    index += 1;
                }
            }
        }
        return (chars.get(index + 1) == Some(&'!')).then_some((name, index + 2));
    }
    let length = chars[at..]
        .iter()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.'))
        .count();
    (length > 0 && chars.get(at + length) == Some(&'!'))
        .then(|| (chars[at..at + length].iter().collect(), at + length + 1))
}

fn ref_text(fixed_column: bool, column: i64, fixed_row: bool, row: i64) -> String {
    format!(
        "{}{}{}{}",
        if fixed_column { "$" } else { "" },
        super::address::column_name(i32::try_from(column).unwrap_or(i32::MAX)),
        if fixed_row { "$" } else { "" },
        row
    )
}

/// A formula with its references to `tab` moved by `moves` (rows or
/// columns inserted or deleted); a reference whose cells were all deleted
/// becomes `#REF!`, as the engine writes it. Unprefixed references are on
/// `tab` when `own_tab`. Text in quotes is left alone.
fn shift_formula(
    formula: &str,
    tab: &str,
    own_tab: bool,
    moves: &[crate::discussion::anchors::GridMove],
) -> String {
    use crate::discussion::anchors::follow_span;
    let chars = formula.chars().collect::<Vec<_>>();
    let mut out = String::with_capacity(formula.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c == '"' {
            // A string literal, with "" for a quote inside it.
            out.push(c);
            index += 1;
            while index < chars.len() {
                out.push(chars[index]);
                if chars[index] == '"' {
                    if chars.get(index + 1) == Some(&'"') {
                        out.push('"');
                        index += 2;
                        continue;
                    }
                    index += 1;
                    break;
                }
                index += 1;
            }
            continue;
        }
        let boundary = index == 0 || {
            let before = chars[index - 1];
            !(before.is_alphanumeric() || matches!(before, '_' | '.' | '$' | '\''))
        };
        if boundary {
            let (sheet, start) = match sheet_prefix(&chars, index) {
                Some((name, start)) => (Some(name), start),
                None => (None, index),
            };
            if let Some(first) = cell_ref(&chars, start) {
                let second = (chars.get(first.end) == Some(&':'))
                    .then(|| cell_ref(&chars, first.end + 1))
                    .flatten();
                let end = second.as_ref().map_or(first.end, |second| second.end);
                let applies = match &sheet {
                    Some(name) => name.eq_ignore_ascii_case(tab),
                    None => own_tab,
                };
                if !applies {
                    out.extend(&chars[index..end]);
                    index = end;
                    continue;
                }
                let last = second.as_ref().unwrap_or(&first);
                // Spans count from 0, as moves do.
                let mut rows = Some((first.row.min(last.row) - 1, first.row.max(last.row) - 1));
                let mut columns = Some((
                    first.column.min(last.column) - 1,
                    first.column.max(last.column) - 1,
                ));
                for step in moves {
                    let span = if step.rows { &mut rows } else { &mut columns };
                    *span = span.and_then(|(from, to)| follow_span(from, to, step));
                }
                out.extend(&chars[index..start]);
                match (rows, columns) {
                    (Some(rows), Some(columns)) => {
                        out.push_str(&ref_text(
                            first.column_fixed,
                            columns.0 + 1,
                            first.row_fixed,
                            rows.0 + 1,
                        ));
                        if let Some(second) = &second {
                            out.push(':');
                            out.push_str(&ref_text(
                                second.column_fixed,
                                columns.1 + 1,
                                second.row_fixed,
                                rows.1 + 1,
                            ));
                        }
                    }
                    _ => out.push_str("#REF!"),
                }
                index = end;
                continue;
            }
        }
        out.push(c);
        index += 1;
    }
    out
}

/// Tells the sheet's readers its proposals changed (ids only).
async fn announce(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    space_id: Uuid,
    sheet_id: Uuid,
    actor: Uuid,
) -> Result<(), AppError> {
    events::emit(
        transaction,
        NewEvent::thing(
            &catalog::SHEET_PROPOSALS_CHANGED,
            space_id,
            "sheet",
            sheet_id,
        )
        .actor(actor)
        .workspace(workspace_id),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discussion::anchors::GridMove;

    fn cell(tab_id: Uuid, row: i32, column: i32) -> ProposedCell {
        ProposedCell {
            tab_id,
            tab: "Plan".to_owned(),
            row,
            column,
            before: String::new(),
            after: "x".to_owned(),
            gone: false,
        }
    }

    #[test]
    fn proposed_formulas_follow_moved_rows_and_columns() {
        let insert_row = |at, count| GridMove {
            rows: true,
            insert: true,
            at,
            count,
        };
        let delete_column = |at, count| GridMove {
            rows: false,
            insert: false,
            at,
            count,
        };
        // A row inserted above row 2 (index 1): A2 is now A3.
        assert_eq!(
            shift_formula("=A2*2", "Plan", true, &[insert_row(1, 1)]),
            "=A3*2"
        );
        assert_eq!(
            shift_formula("=SUM($A$1:B4)+LOG10(C9)", "Plan", true, &[insert_row(1, 2)]),
            "=SUM($A$1:B6)+LOG10(C11)"
        );
        // Text in quotes and other tabs stay; this tab named outright moves.
        assert_eq!(
            shift_formula(
                "=\"A2\"&Other!A2&Plan!A2&'Plan'!B2",
                "Plan",
                true,
                &[insert_row(0, 1)]
            ),
            "=\"A2\"&Other!A2&Plan!A3&'Plan'!B3"
        );
        assert_eq!(
            shift_formula("=A2", "Plan", false, &[insert_row(0, 1)]),
            "=A2"
        );
        // A deleted column: what was in it is gone, what was after it moves left.
        assert_eq!(
            shift_formula("=B1+C1+SUM(A1:C1)", "Plan", true, &[delete_column(1, 1)]),
            "=#REF!+B1+SUM(A1:B1)"
        );
    }

    #[test]
    fn waiting_cells_move_with_rows_and_columns() {
        let tab = Uuid::from_u128(1);
        let other = Uuid::from_u128(2);
        let mut cells = vec![
            cell(tab, 5, 2),
            cell(tab, 2, 2),
            cell(other, 5, 2),
            cell(tab, 7, 1),
        ];
        // Two rows inserted above row 4 (anchors count from 0), then row 9 deleted.
        let moves = [
            GridMove {
                rows: true,
                insert: true,
                at: 3,
                count: 2,
            },
            GridMove {
                rows: true,
                insert: false,
                at: 8,
                count: 1,
            },
        ];
        assert!(move_cells(&mut cells, tab, &moves));
        assert_eq!(
            (cells[0].row, cells[0].column, cells[0].gone),
            (7, 2, false)
        );
        assert_eq!((cells[1].row, cells[1].gone), (2, false));
        assert_eq!((cells[2].row, cells[2].gone), (5, false));
        // Row 7 became 9, which the delete took.
        assert!(cells[3].gone);
        // A column inserted to the right of everything moves nothing.
        let far = [GridMove {
            rows: false,
            insert: true,
            at: 20,
            count: 1,
        }];
        assert!(!move_cells(&mut cells, tab, &far));
    }
}

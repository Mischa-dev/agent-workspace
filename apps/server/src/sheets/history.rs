//! A sheet's history (C1.13): any revision can be shown and put back.
//!
//! Each revision keeps the inputs it changed (`sheet_revisions.cells`, by
//! tab id and position, a cleared cell as null). A snapshot of every tab and
//! input (`sheet_snapshots`) is kept when the sheet is made, whenever its
//! tabs change (added, removed, renamed, moved, frozen rows, widths), when
//! one write changes too many cells to keep, and every 25 revisions. A
//! revision is the latest snapshot at or before it with the inputs of the
//! revisions after it laid over. Restoring writes those inputs back as a
//! new revision through the one write path, so values are recomputed and
//! the restore is itself a revision.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{Applied, Change, Problem, Sheet, address::column_name};
use crate::error::AppError;

/// One input a revision changed: tab id, row, column, and the new input
/// (None when cleared).
pub type Delta = (Uuid, i32, i32, Option<String>);

/// How many revisions may pass between snapshots.
const SNAPSHOT_EVERY: i64 = 25;
/// The most inputs one revision keeps; past it the revision is a snapshot.
const MAX_DELTA: usize = 5_000;

/// Keeps what revision `revision` changed, and a snapshot when it's due.
/// Call after the revision's row and cells are written.
pub async fn record(
    connection: &mut PgConnection,
    sheet_id: Uuid,
    revision: i64,
    tabs_changed: bool,
    delta: &[Delta],
) -> Result<(), AppError> {
    let kept = !tabs_changed && delta.len() <= MAX_DELTA;
    if kept {
        let cells = delta
            .iter()
            .map(|(tab, row, column, input)| json!([tab, row, column, input]))
            .collect::<Vec<_>>();
        sqlx::query("UPDATE sheet_revisions SET cells = $3 WHERE sheet_id = $1 AND revision = $2")
            .bind(sheet_id)
            .bind(revision)
            .bind(Json::Array(cells))
            .execute(&mut *connection)
            .await?;
    }
    let last = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(revision) FROM sheet_snapshots WHERE sheet_id = $1",
    )
    .bind(sheet_id)
    .fetch_one(&mut *connection)
    .await?;
    if !kept || last.is_none_or(|last| revision - last >= SNAPSHOT_EVERY) {
        snapshot(connection, sheet_id, revision).await?;
    }
    Ok(())
}

/// Keeps every tab and input of the sheet as it is now, as `revision`.
pub async fn snapshot(
    connection: &mut PgConnection,
    sheet_id: Uuid,
    revision: i64,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO sheet_snapshots (workspace_id, space_id, sheet_id, revision, state)
        SELECT project.organization_id, sheet.project_id, sheet.id, $2,
               jsonb_build_object('tabs', COALESCE((
                   SELECT jsonb_agg(jsonb_build_object(
                       'id', tab.id, 'name', tab.name, 'frozenRows', tab.frozen_rows,
                       'columnWidths', tab.column_widths,
                       'cells', COALESCE((
                           SELECT jsonb_agg(jsonb_build_array(cell.row_number, cell.column_number, cell.input)
                                            ORDER BY cell.row_number, cell.column_number)
                           FROM sheet_cells cell WHERE cell.tab_id = tab.id), '[]'::jsonb)
                   ) ORDER BY tab.position, tab.id)
                   FROM sheet_tabs tab WHERE tab.sheet_id = sheet.id), '[]'::jsonb))
        FROM sheets sheet JOIN projects project ON project.id = sheet.project_id
        WHERE sheet.id = $1
        ON CONFLICT (sheet_id, revision) DO UPDATE SET state = EXCLUDED.state
        "#,
    )
    .bind(sheet_id)
    .bind(revision)
    .execute(connection)
    .await?;
    Ok(())
}

/// A sheet's tabs and inputs at one revision.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub tabs: Vec<TabState>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TabState {
    pub id: Uuid,
    pub name: String,
    pub frozen_rows: i32,
    pub column_widths: Json,
    /// (row, column, input), in order.
    pub cells: Vec<(i32, i32, String)>,
}

impl State {
    /// Lays one revision's inputs over the state.
    fn apply(&mut self, delta: &[(Uuid, i32, i32, Option<String>)]) {
        for tab in &mut self.tabs {
            let changes = delta
                .iter()
                .filter(|(id, ..)| *id == tab.id)
                .collect::<Vec<_>>();
            if changes.is_empty() {
                continue;
            }
            let mut cells: BTreeMap<(i32, i32), String> = tab
                .cells
                .drain(..)
                .map(|(row, column, input)| ((row, column), input))
                .collect();
            for (_, row, column, input) in changes {
                match input {
                    Some(input) => cells.insert((*row, *column), input.clone()),
                    None => cells.remove(&(*row, *column)),
                };
            }
            tab.cells = cells
                .into_iter()
                .map(|((row, column), input)| (row, column, input))
                .collect();
        }
    }

    /// As `history_show` returns it: tabs by name, inputs by address.
    pub fn to_json(&self, revision: i64) -> Json {
        json!({
            "revision": revision,
            "tabs": self.tabs.iter().map(|tab| {
                let cells = tab
                    .cells
                    .iter()
                    .map(|(row, column, input)| {
                        (format!("{}{row}", column_name(*column)), Json::String(input.clone()))
                    })
                    .collect::<serde_json::Map<_, _>>();
                json!({ "name": tab.name, "cells": cells })
            }).collect::<Vec<_>>(),
        })
    }
}

/// The sheet's tabs and inputs at `revision`, or None when its history
/// doesn't reach back that far (revisions from before snapshots were kept).
pub async fn state_at(
    connection: &mut PgConnection,
    sheet_id: Uuid,
    revision: i64,
) -> Result<Option<State>, AppError> {
    let Some((base, sqlx::types::Json(mut state))) =
        sqlx::query_as::<_, (i64, sqlx::types::Json<State>)>(
            "SELECT revision, state FROM sheet_snapshots \
             WHERE sheet_id = $1 AND revision <= $2 ORDER BY revision DESC LIMIT 1",
        )
        .bind(sheet_id)
        .bind(revision)
        .fetch_optional(&mut *connection)
        .await?
    else {
        return Ok(None);
    };
    let later = sqlx::query_as::<_, (i64, Option<sqlx::types::Json<Vec<Delta>>>)>(
        "SELECT revision, cells FROM sheet_revisions \
         WHERE sheet_id = $1 AND revision > $2 AND revision <= $3 ORDER BY revision",
    )
    .bind(sheet_id)
    .bind(base)
    .bind(revision)
    .fetch_all(&mut *connection)
    .await?;
    if i64::try_from(later.len()).unwrap_or(i64::MAX) != revision - base {
        return Ok(None);
    }
    for (_, cells) in later {
        let Some(sqlx::types::Json(delta)) = cells else {
            return Ok(None);
        };
        state.apply(&delta);
    }
    Ok(Some(state))
}

/// The changes that make the sheet as it is now (`now`, with its tabs'
/// current settings) into `target`: tabs added, removed, renamed and put
/// in order, their settings, and every input that differs.
pub fn changes_to(now: &State, target: &State) -> Vec<Change> {
    let mut changes = Vec::new();
    let surviving = |id: Uuid| target.tabs.iter().any(|tab| tab.id == id);
    let any_survives = now.tabs.iter().any(|tab| surviving(tab.id));
    let removals = now
        .tabs
        .iter()
        .filter(|tab| !surviving(tab.id))
        .map(|tab| Change::RemoveTab {
            tab: Some(tab.id.to_string()),
        })
        .collect::<Vec<_>>();
    let additions = target
        .tabs
        .iter()
        .filter(|tab| !now.tabs.iter().any(|current| current.id == tab.id))
        .map(|tab| Change::AddTab {
            name: tab.name.clone(),
        })
        .collect::<Vec<_>>();
    // A sheet keeps at least one tab: when none survives, add first.
    if any_survives {
        changes.extend(removals);
        changes.extend(additions);
    } else {
        changes.extend(additions);
        changes.extend(removals);
    }
    for (position, tab) in target.tabs.iter().enumerate() {
        let current = now.tabs.iter().find(|current| current.id == tab.id);
        // Surviving tabs are named by id until renamed; added ones by name.
        let reference = match current {
            Some(current) if current.name != tab.name => {
                changes.push(Change::RenameTab {
                    tab: Some(tab.id.to_string()),
                    name: tab.name.clone(),
                });
                Some(tab.name.clone())
            }
            Some(_) => Some(tab.id.to_string()),
            None => Some(tab.name.clone()),
        };
        changes.push(Change::MoveTab {
            tab: reference.clone(),
            position: i32::try_from(position + 1).unwrap_or(i32::MAX),
        });
        let current_frozen = current.map(|current| current.frozen_rows).unwrap_or(1);
        if current_frozen != tab.frozen_rows {
            changes.push(Change::FreezeRows {
                tab: reference.clone(),
                rows: tab.frozen_rows,
            });
        }
        let widths = |state: Option<&TabState>| {
            state
                .and_then(|state| state.column_widths.as_object().cloned())
                .unwrap_or_default()
        };
        let (had, wanted) = (widths(current), widths(Some(tab)));
        for (column, width) in &wanted {
            if had.get(column) != Some(width)
                && let Ok(column) = column.parse::<i32>()
            {
                changes.push(Change::ColumnWidth {
                    tab: reference.clone(),
                    column,
                    width: width.as_i64().and_then(|width| i32::try_from(width).ok()),
                });
            }
        }
        for column in had.keys() {
            if !wanted.contains_key(column)
                && let Ok(column) = column.parse::<i32>()
            {
                changes.push(Change::ColumnWidth {
                    tab: reference.clone(),
                    column,
                    width: None,
                });
            }
        }
        let had_cells: BTreeMap<(i32, i32), &str> = current
            .map(|current| {
                current
                    .cells
                    .iter()
                    .map(|(row, column, input)| ((*row, *column), input.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        let wanted_cells: BTreeMap<(i32, i32), &str> = tab
            .cells
            .iter()
            .map(|(row, column, input)| ((*row, *column), input.as_str()))
            .collect();
        for (&(row, column), input) in &wanted_cells {
            if had_cells.get(&(row, column)) != Some(input) {
                changes.push(Change::Cell {
                    tab: reference.clone(),
                    row,
                    column,
                    input: (*input).to_owned(),
                    if_revision: None,
                });
            }
        }
        for &(row, column) in had_cells.keys() {
            if !wanted_cells.contains_key(&(row, column)) {
                changes.push(Change::Cell {
                    tab: reference.clone(),
                    row,
                    column,
                    input: String::new(),
                    if_revision: None,
                });
            }
        }
    }
    changes
}

/// The sheet's tabs and inputs as they are now.
async fn current(connection: &mut PgConnection, sheet_id: Uuid) -> Result<State, AppError> {
    let tabs = super::tabs(&mut *connection, sheet_id).await?;
    let cells = super::cells(&mut *connection, sheet_id).await?;
    Ok(State {
        tabs: tabs
            .into_iter()
            .map(|tab| TabState {
                id: tab.id,
                cells: cells
                    .iter()
                    .filter(|cell| cell.tab_id == tab.id)
                    .map(|cell| (cell.row_number, cell.column_number, cell.input.clone()))
                    .collect(),
                name: tab.name,
                frozen_rows: tab.frozen_rows,
                column_widths: tab.column_widths,
            })
            .collect(),
    })
}

/// Why a restore wasn't done.
#[derive(Debug)]
pub enum Refusal {
    /// The history doesn't reach back to that revision.
    NotKept,
    /// The write was refused (a limit, a name).
    Problem(Problem),
}

/// Puts the locked sheet's tabs and inputs back as they were at
/// `revision`, as a new revision by `actor_id`.
pub async fn restore(
    connection: &mut PgConnection,
    sheet: &Sheet,
    actor_id: Uuid,
    revision: i64,
) -> Result<Result<Applied, Refusal>, AppError> {
    let Some(target) = state_at(&mut *connection, sheet.id, revision).await? else {
        return Ok(Err(Refusal::NotKept));
    };
    let now = current(&mut *connection, sheet.id).await?;
    let changes = changes_to(&now, &target);
    crate::events::ensure_actor(&mut *connection, actor_id).await?;
    Ok(super::apply_unbounded(
        &mut *connection,
        sheet,
        actor_id,
        &changes,
        0,
        &super::IronCalc,
    )
    .await?
    .map_err(Refusal::Problem))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: u128, name: &str, cells: &[(i32, i32, &str)]) -> TabState {
        TabState {
            id: Uuid::from_u128(id),
            name: name.to_owned(),
            frozen_rows: 1,
            column_widths: json!({}),
            cells: cells
                .iter()
                .map(|(row, column, input)| (*row, *column, (*input).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn revisions_lay_over_their_snapshot() {
        let mut state = State {
            tabs: vec![tab(1, "Budget", &[(1, 1, "Item"), (2, 1, "Rent")])],
        };
        state.apply(&[
            (Uuid::from_u128(1), 2, 1, None),
            (Uuid::from_u128(1), 2, 2, Some("1200".into())),
        ]);
        assert_eq!(
            state.tabs[0].cells,
            vec![(1, 1, "Item".into()), (2, 2, "1200".into())]
        );
        let shown = state.to_json(3);
        assert_eq!(shown["tabs"][0]["cells"]["B2"], "1200");
    }

    #[test]
    fn changes_put_back_inputs_and_tabs() {
        let now = State {
            tabs: vec![
                tab(
                    1,
                    "Costs",
                    &[(1, 1, "Item"), (2, 1, "Rent"), (3, 1, "Food")],
                ),
                tab(2, "Notes", &[]),
            ],
        };
        let target = State {
            tabs: vec![tab(1, "Budget", &[(1, 1, "Item"), (2, 1, "Lease")])],
        };
        let changes = changes_to(&now, &target);
        let shorts = changes.iter().map(Change::short).collect::<Vec<_>>();
        assert_eq!(
            shorts,
            vec!["removeTab", "renameTab", "moveTab", "cell", "cell"]
        );
    }
}

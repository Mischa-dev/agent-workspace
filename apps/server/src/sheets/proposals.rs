//! Proposals on sheets (C1.11): someone whose role only suggests on sheets
//! writes cells as usual, and the write is kept as a proposal instead.
//!
//! A proposal is the cells it would set, each with the input it read
//! (`before`) and the one it proposes (`after`), worked out by running the
//! write inside a savepoint and rolling it back, so a row by key or a
//! formula lands exactly where the real write would put it. Accepting
//! applies the chosen cells as the author's write, and is refused when any
//! of them changed since, so it never undoes a change the author hadn't
//! seen. Only cell contents can be proposed; tabs, rows and looks need
//! edit access.

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

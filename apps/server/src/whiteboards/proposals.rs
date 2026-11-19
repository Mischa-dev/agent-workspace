//! Proposals on whiteboards (C1.11): someone whose role only suggests on
//! boards draws as usual, and the change is kept as a proposal instead.
//!
//! A proposal is the elements it would set, each with the copy it read
//! (`before`, absent for a new element) and the one it proposes (`after`),
//! in Excalidraw's own JSON. Agents' operations are run on the scene in
//! memory and the elements they change are proposed; a browser in suggest
//! mode sends the elements it changed. Accepting writes the chosen
//! elements through the board's one write path (`store::write_elements`)
//! as the author's change, and is refused when any of them changed since,
//! so it never undoes a change the author hadn't seen. Images can't be
//! proposed: a new image needs edit access to put its file on the board.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    BoardAccess, Caller,
    elements::{self, Element},
    render,
    scene::{self, Object},
    store,
};
use crate::{
    error::AppError,
    events::{self, NewEvent, catalog},
    memory::Writer,
    page_docs::suggestions::{Decided, Decision, Refusal},
};

/// The most elements one proposal sets.
pub const MAX_PROPOSED_ELEMENTS: usize = 500;
/// The most proposals one author keeps waiting on one board.
const MAX_PENDING_PER_AUTHOR: i64 = 20;
/// A preview larger than this (as SVG) isn't kept; the change still is.
const MAX_PREVIEW_BYTES: usize = 300 * 1024;

/// One element a proposal sets.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProposedElement {
    pub id: String,
    /// The element's type (rectangle, text, arrow…).
    #[serde(rename = "type")]
    pub kind: String,
    /// add, change or remove.
    pub action: String,
    /// The copy it read; absent for a new element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    /// The copy it proposes (a removal is the element marked deleted).
    pub after: Value,
}

/// What proposing came to.
#[derive(Debug)]
pub enum Proposed {
    Made {
        id: Uuid,
        elements: Vec<ProposedElement>,
    },
    /// Nothing would change.
    Unchanged,
    /// Images, too much at once, or too many waiting.
    Refused(&'static str),
}

fn version_of(element: &Value) -> (i64, i64) {
    (
        element.get("version").and_then(Value::as_i64).unwrap_or(0),
        element
            .get("versionNonce")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    )
}

fn is_deleted(element: &Value) -> bool {
    element
        .get("isDeleted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Keeps `incoming` (elements as an edit would write them) as a proposal on
/// `board` by `author`. The caller has checked the author may suggest on
/// the board and locked it (`store::lock`).
pub async fn propose(
    transaction: &mut Transaction<'_, Postgres>,
    author: &Caller,
    board: &BoardAccess,
    scene_seq: i64,
    incoming: Vec<Element>,
    operation: &str,
    note: Option<&str>,
) -> Result<Proposed, AppError> {
    if incoming.len() > MAX_PROPOSED_ELEMENTS {
        return Ok(Proposed::Refused("Propose up to 500 elements at a time."));
    }
    let rows = store::load_rows(transaction, board.id).await?;
    let stored = rows
        .iter()
        .map(|row| (scene::id(row).to_owned(), row))
        .collect::<HashMap<_, _>>();
    let mut latest = Vec::<Element>::new();
    for element in incoming {
        latest.retain(|kept| kept.id != element.id);
        latest.push(element);
    }
    let mut proposed = Vec::with_capacity(latest.len());
    for element in latest {
        let before = stored
            .get(&element.id)
            .map(|row| Value::Object((*row).clone()));
        let after = Value::Object(element.data.clone());
        // A copy that wouldn't win, or a new element already gone, isn't a change.
        let wins = before.as_ref().is_none_or(|before| {
            let (version, nonce) = version_of(before);
            elements::incoming_wins(
                (element.version, element.version_nonce),
                (i32::try_from(version).unwrap_or(i32::MAX), nonce),
            )
        });
        if !wins || (before.is_none() && element.is_deleted) {
            continue;
        }
        let action = match &before {
            None => "add",
            Some(_) if element.is_deleted => "remove",
            Some(before) if is_deleted(before) => "add",
            Some(_) => "change",
        };
        if element.kind == "image" && action != "remove" {
            let moved_only = before.as_ref().is_some_and(|before| {
                before.get("fileId") == after.get("fileId") && !is_deleted(before)
            });
            if !moved_only {
                return Ok(Proposed::Refused(
                    "Your role here suggests changes to whiteboards: you can propose shapes, \
                     text and arrows, but adding images needs edit access.",
                ));
            }
        }
        proposed.push(ProposedElement {
            id: element.id,
            kind: element.kind,
            action: action.to_owned(),
            before,
            after,
        });
    }
    if proposed.is_empty() {
        return Ok(Proposed::Unchanged);
    }
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM proposals \
         WHERE thing_kind = 'whiteboard' AND thing_id = $1 AND author_actor_id = $2 \
           AND status = 'pending'",
    )
    .bind(board.id)
    .bind(author.actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if pending >= MAX_PENDING_PER_AUTHOR {
        return Ok(Proposed::Refused(
            "You have too many proposals waiting on this board. Wait for some to be decided, or \
             withdraw them.",
        ));
    }
    let preview = preview(&rows, &proposed);
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO proposals (
            id, workspace_id, space_id, thing_kind, thing_id, operation, author_actor_id,
            via_oauth_grant_id, note, base_version, change, preview
        )
        VALUES ($1, $2, $3, 'whiteboard', $4, $5, $6, $7, $8, $9,
                jsonb_build_object('elements', $10::jsonb), $11)
        "#,
    )
    .bind(id)
    .bind(board.organization_id)
    .bind(board.project_id)
    .bind(board.id)
    .bind(operation)
    .bind(author.actor_id)
    .bind(author.via_grant_id)
    .bind(note)
    .bind(scene_seq.max(1))
    .bind(serde_json::to_value(&proposed).map_err(|_| AppError::Internal("proposed elements"))?)
    .bind(preview)
    .execute(&mut **transaction)
    .await?;
    announce(
        transaction,
        board.organization_id,
        board.project_id,
        board.id,
        author.actor_id,
    )
    .await?;
    Ok(Proposed::Made {
        id,
        elements: proposed,
    })
}

/// The changed part of the board before and after, as SVG, cropped to the
/// elements the proposal touches. Images are drawn as placeholders; a
/// preview too big to keep is left out.
fn preview(rows: &[Object], proposed: &[ProposedElement]) -> Value {
    let touched = proposed
        .iter()
        .map(|element| element.id.clone())
        .collect::<Vec<_>>();
    let mut after = rows.to_vec();
    for element in proposed {
        let Value::Object(object) = &element.after else {
            continue;
        };
        match after.iter_mut().find(|row| scene::id(row) == element.id) {
            Some(row) => *row = object.clone(),
            None => after.push(object.clone()),
        }
    }
    let draw = |elements: &[Object]| {
        let shown = touched
            .iter()
            .filter(|id| {
                elements
                    .iter()
                    .any(|element| scene::id(element) == id.as_str() && !scene::is_deleted(element))
            })
            .cloned()
            .collect::<Vec<_>>();
        if shown.is_empty() {
            return None;
        }
        let rendered = render::svg(
            elements,
            &HashMap::new(),
            &render::Options {
                only: Some(shown),
                background: false,
                dark: false,
            },
        );
        (rendered.svg.len() <= MAX_PREVIEW_BYTES).then_some(rendered.svg)
    };
    let mut preview = serde_json::Map::new();
    if let Some(svg) = draw(rows) {
        preview.insert("before".to_owned(), Value::String(svg));
    }
    if let Some(svg) = draw(&after) {
        preview.insert("after".to_owned(), Value::String(svg));
    }
    Value::Object(preview)
}

/// Accepts (all of it, or the elements at `parts`), rejects or withdraws a
/// proposal on `board_id`. `may_edit` is whether `decider` edits the board
/// directly; accepting and rejecting need it, and only the author
/// withdraws.
pub async fn decide(
    transaction: &mut Transaction<'_, Postgres>,
    decider: Writer,
    may_edit: bool,
    board_id: Uuid,
    proposal_id: Uuid,
    decision: Decision,
    parts: Option<&[u16]>,
) -> Result<Result<Decided, Refusal>, AppError> {
    let Some((space_id, workspace_id, author_id, via_grant, status, sqlx::types::Json(proposed))) =
        sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                Uuid,
                Option<Uuid>,
                String,
                sqlx::types::Json<Vec<ProposedElement>>,
            ),
        >(
            "SELECT space_id, workspace_id, author_actor_id, via_oauth_grant_id, status, \
                    change -> 'elements' \
             FROM proposals WHERE id = $1 AND thing_kind = 'whiteboard' AND thing_id = $2 \
             FOR UPDATE",
        )
        .bind(proposal_id)
        .bind(board_id)
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
                "Deciding a proposal needs edit access to the whiteboard. Ask someone who edits it.",
            )));
        }
        Decision::Withdraw | Decision::Reject if parts.is_some() => {
            return Ok(Err(Refusal::NotYours(
                "Parts are for accepting; a rejection or withdrawal takes the whole proposal.",
            )));
        }
        _ => {}
    }
    let mut scene_seq = store::lock(transaction, board_id).await?;
    let status = match decision {
        Decision::Accept => {
            let deleted = sqlx::query_scalar::<_, bool>(
                "SELECT deleted_at IS NOT NULL FROM whiteboards WHERE id = $1",
            )
            .bind(board_id)
            .fetch_one(&mut **transaction)
            .await?;
            if deleted {
                return Ok(Err(Refusal::Archived));
            }
            let chosen: Vec<&ProposedElement> = match parts {
                None => proposed.iter().collect(),
                Some(parts) => {
                    let mut chosen = Vec::with_capacity(parts.len());
                    for part in parts {
                        match proposed.get(usize::from(*part)) {
                            Some(element) if !chosen.contains(&element) => chosen.push(element),
                            Some(_) => {}
                            None => {
                                return Ok(Err(Refusal::Stale(format!(
                                    "It has {} elements, numbered from 0; there's no part {part}.",
                                    proposed.len()
                                ))));
                            }
                        }
                    }
                    chosen
                }
            };
            if chosen.is_empty() {
                return Ok(Err(Refusal::Stale(
                    "Choose at least one of its elements to accept.".to_owned(),
                )));
            }
            let ids = chosen
                .iter()
                .map(|element| element.id.clone())
                .collect::<Vec<_>>();
            let now = sqlx::query_as::<_, (String, i32, i64)>(
                "SELECT element_id, version, version_nonce FROM whiteboard_elements \
                 WHERE whiteboard_id = $1 AND element_id = ANY($2)",
            )
            .bind(board_id)
            .bind(&ids)
            .fetch_all(&mut **transaction)
            .await?
            .into_iter()
            .map(|(id, version, nonce)| (id, (i64::from(version), nonce)))
            .collect::<HashMap<_, _>>();
            let mut writes = Vec::with_capacity(chosen.len());
            for element in &chosen {
                let read = element.before.as_ref().map(version_of);
                if now.get(&element.id).copied() != read {
                    return Ok(Err(Refusal::Stale(format!(
                        "The {} {} has changed since it was proposed.",
                        element.kind, element.id
                    ))));
                }
                writes.push(
                    elements::parse(element.after.clone())
                        .map_err(|_| AppError::Internal("proposed element"))?,
                );
            }
            // The elements are the author's: the board's history says so.
            let author = author_caller(transaction, author_id, via_grant).await?;
            let saved = store::save_revision(transaction, &author, board_id, false, None).await?;
            let outcome =
                store::write_elements(transaction, &author, board_id, scene_seq, writes).await?;
            if !outcome.rejected.is_empty() {
                return Ok(Err(Refusal::Stale(
                    "Part of the board changed while it was being accepted.".to_owned(),
                )));
            }
            if let Some(revision) = saved {
                store::updated(transaction, &author, board_id, revision).await?;
            }
            scene_seq = outcome.scene_seq;
            if chosen.len() == proposed.len() {
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
                applied_version = $4, updated_at = now() \
         WHERE id = $1",
    )
    .bind(proposal_id)
    .bind(status)
    .bind(decider.actor_id)
    .bind(applied.then_some(scene_seq))
    .execute(&mut **transaction)
    .await?;
    announce(
        transaction,
        workspace_id,
        space_id,
        board_id,
        decider.actor_id,
    )
    .await?;
    Ok(Ok(Decided {
        status,
        version: scene_seq,
    }))
}

/// The author as the board's write path records them.
async fn author_caller(
    transaction: &mut Transaction<'_, Postgres>,
    author_id: Uuid,
    via_grant_id: Option<Uuid>,
) -> Result<Caller, AppError> {
    let kind = sqlx::query_scalar::<_, String>("SELECT kind FROM actors WHERE id = $1")
        .bind(author_id)
        .fetch_one(&mut **transaction)
        .await?;
    Ok(Caller {
        actor_id: author_id,
        actor_kind: crate::conversations::parse_actor_kind(&kind)?,
        via_grant_id,
        connection_id: None,
        confined_project: None,
        channel: "proposal",
        authentication_class: "background_job",
        session_id: None,
    })
}

/// Tells the board's readers its proposals changed (ids only).
async fn announce(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    space_id: Uuid,
    board_id: Uuid,
    actor: Uuid,
) -> Result<(), AppError> {
    events::emit(
        transaction,
        NewEvent::thing(
            &catalog::WHITEBOARD_PROPOSALS_CHANGED,
            space_id,
            "whiteboard",
            board_id,
        )
        .actor(actor)
        .workspace(workspace_id),
    )
    .await?;
    Ok(())
}

//! Proposals (C1.7): one record for a change someone made to a thing they
//! may only suggest on (or asked to propose), waiting for someone who edits
//! the thing to accept or reject it. The author may withdraw it.
//!
//! The row is the same for every kind; the `change` is the kind's own and
//! says what each part read, so accepting can never undo a change the
//! author hadn't seen. Each kind keeps, checks and applies its change
//! through its own write path: pages through `page_docs::suggestions`
//! (block edits by lasting id and hash). Sheets, boards and records join in
//! C1.11; until then a proposal of another kind can't be decided here.
//!
//! `proposal_list` and `proposal_decide` (in `ops.rs`) are the one way to
//! read and decide them; the page tools and routes for suggestions stay as
//! aliases.

pub mod ops;

use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{access, error::AppError, memory::Writer, page_docs::suggestions};

/// The kinds a proposal can be about today.
pub const PROPOSABLE_KINDS: &[&str] = &["page"];

/// A proposal as it's listed.
#[derive(Clone, Debug, Serialize, FromRow, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProposalView {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// What it changes, by kind (`page`) and id.
    pub thing_kind: String,
    #[schemars(with = "String")]
    pub thing_id: Uuid,
    /// The thing's title, for a list.
    pub thing_title: Option<String>,
    #[schemars(with = "String")]
    pub space_id: Uuid,
    /// The operation the change would have run.
    pub operation: String,
    #[schemars(with = "String")]
    pub author_id: Uuid,
    pub author: Option<String>,
    pub note: Option<String>,
    /// The thing's version the author read.
    pub base_version: i64,
    /// The change in the kind's shape: for a page, `{"edits": [...]}` by
    /// lasting block id, each with the Markdown it replaces (`before`).
    #[schemars(with = "serde_json::Value")]
    pub change: sqlx::types::Json<serde_json::Value>,
    /// pending, accepted, partly_accepted, rejected, withdrawn or stale.
    pub status: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub decision_note: Option<String>,
    /// The thing's version accepting it made.
    pub applied_version: Option<i64>,
    pub created_at: String,
}

const SELECT: &str = r#"
    SELECT proposal.id, proposal.thing_kind, proposal.thing_id,
           CASE proposal.thing_kind
             WHEN 'page' THEN (SELECT page.title FROM project_pages page WHERE page.id = proposal.thing_id)
           END AS thing_title,
           proposal.space_id, proposal.operation, proposal.author_actor_id AS author_id,
           author.display_name AS author, proposal.note, proposal.base_version,
           proposal.change, proposal.status, decider.display_name AS decided_by,
           to_char(proposal.decided_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS decided_at,
           proposal.decision_note, proposal.applied_version,
           to_char(proposal.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS created_at
    FROM proposals proposal
    LEFT JOIN project_memberships author
      ON author.project_id = proposal.space_id AND author.actor_id = proposal.author_actor_id
    LEFT JOIN project_memberships decider
      ON decider.project_id = proposal.space_id AND decider.actor_id = proposal.decided_by_actor_id
"#;

/// Which proposals to list.
#[derive(Clone, Copy, Debug)]
pub enum Listing {
    /// The ones on one thing.
    Thing(&'static str, Uuid),
    /// The ones the viewer made.
    Mine,
}

/// Proposals the viewer can see (every row is checked against the thing it
/// changes), newest first. `pending_only` leaves out decided ones.
pub async fn list(
    connection: &mut PgConnection,
    viewer: Uuid,
    listing: Listing,
    pending_only: bool,
    limit: i64,
) -> Result<Vec<ProposalView>, AppError> {
    let (kind, thing, author) = match listing {
        Listing::Thing(kind, id) => (Some(kind), Some(id), None),
        Listing::Mine => (None, None, Some(viewer)),
    };
    Ok(sqlx::query_as::<_, ProposalView>(&format!(
        "{SELECT}
         WHERE ($1::text IS NULL OR (proposal.thing_kind = $1 AND proposal.thing_id = $2))
           AND ($3::uuid IS NULL OR proposal.author_actor_id = $3)
           AND (NOT $4 OR proposal.status = 'pending')
           AND subject_visible_to(proposal.thing_kind, proposal.thing_id, $5)
         ORDER BY proposal.created_at DESC, proposal.id DESC
         LIMIT $6"
    ))
    .bind(kind)
    .bind(thing)
    .bind(author)
    .bind(pending_only)
    .bind(viewer)
    .bind(limit)
    .fetch_all(connection)
    .await?)
}

/// One proposal, if the viewer can see the thing it changes.
pub async fn get(
    connection: &mut PgConnection,
    viewer: Uuid,
    id: Uuid,
) -> Result<Option<ProposalView>, AppError> {
    Ok(sqlx::query_as::<_, ProposalView>(&format!(
        "{SELECT} WHERE proposal.id = $1 AND subject_visible_to(proposal.thing_kind, proposal.thing_id, $2)"
    ))
    .bind(id)
    .bind(viewer)
    .fetch_optional(connection)
    .await?)
}

/// What someone decides.
pub use suggestions::Decision;

/// Accepts, rejects or withdraws a proposal as `caller`. Accepting and
/// rejecting need edit on the thing (`access::write_mode` answers
/// `Direct`); only the author withdraws. Returns the proposal's new status
/// and the thing's version.
pub async fn decide(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &access::Caller,
    writer: Writer,
    proposal_id: Uuid,
    decision: Decision,
) -> Result<Result<suggestions::Decided, suggestions::Refusal>, AppError> {
    let Some((kind, thing_id, space_id)) = sqlx::query_as::<_, (String, Uuid, Uuid)>(
        "SELECT thing_kind, thing_id, space_id FROM proposals WHERE id = $1",
    )
    .bind(proposal_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(Err(suggestions::Refusal::NotFound));
    };
    match kind.as_str() {
        "page" => {
            let creator = suggestions::page_creator(transaction, thing_id).await?;
            let may_edit = access::write_mode(
                transaction,
                caller,
                "page",
                space_id,
                creator == Some(caller.account_id),
                Some("pages"),
            )
            .await?
            .is_ok_and(|grant| grant.mode == access::WriteMode::Direct);
            suggestions::decide(
                transaction,
                writer,
                may_edit,
                thing_id,
                proposal_id,
                decision,
            )
            .await
        }
        _ => Ok(Err(suggestions::Refusal::NotYours(
            "Proposals on this kind of thing can't be decided yet.",
        ))),
    }
}

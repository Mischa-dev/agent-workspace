//! C2.39: who may message whom, the same for every kind of account.
//!
//! - Sending in a DM needs a shared workspace, checked on every send: a DM
//!   stays readable to its two people after they stop sharing one, but takes
//!   no new messages (`dm_not_shared`).
//! - A block refuses new messages both ways, takes the DM out of the
//!   blocker's list and keeps the blocked account's notifications (mentions
//!   in shared channels included) out of the blocker's inbox. The blocked
//!   account isn't told.
//! - Someone whose `dm_policy` is `ask` gets a stranger's first message as a
//!   request; nothing more reaches them until they accept it (or write back,
//!   which accepts it). A declined request leaves their list.
//!
//! These are the account's own settings, server-wide (R19): `account_blocks`,
//! `dm_requests` and `account_profiles.dm_policy`. Where the DM is stored
//! (K4's direct scope) is the direct module's business; these rules only
//! read its members.

pub mod ops;

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

/// The settings an account can choose for its DMs.
pub const DM_POLICIES: [&str; 2] = ["anyone", "ask"];

const NOT_SHARED: AppError = AppError::Refused {
    field: "dm_not_shared",
    message: "You no longer share a workspace with them, so new messages can't be sent here.",
};
const CLOSED: AppError = AppError::Refused {
    field: "dm_closed",
    message: "New messages can't be sent in this conversation.",
};
const WAITING: AppError = AppError::Refused {
    field: "dm_request_pending",
    message: "They haven't accepted your message request yet. Nothing more is sent until they do.",
};

/// The other person in a DM, or none for a note to self.
async fn other_member(
    tx: &mut Transaction<'_, Postgres>,
    conversation: Uuid,
    account: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM conversation_members \
         WHERE conversation_id = $1 AND actor_id <> $2 \
         ORDER BY actor_id LIMIT 1",
    )
    .bind(conversation)
    .bind(account)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Whether either has blocked the other.
pub(crate) async fn blocked_either_way(
    tx: &mut Transaction<'_, Postgres>,
    one: Uuid,
    other: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM account_blocks \
         WHERE (account_id = $1 AND blocked_account_id = $2) \
            OR (account_id = $2 AND blocked_account_id = $1))",
    )
    .bind(one)
    .bind(other)
    .fetch_one(&mut **tx)
    .await?)
}

/// Whether both are enabled members of one active workspace.
pub(crate) async fn share_a_workspace(
    tx: &mut Transaction<'_, Postgres>,
    one: Uuid,
    other: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM organization_memberships mine
            JOIN organization_memberships theirs
              ON theirs.organization_id = mine.organization_id
             AND theirs.actor_id = $2 AND theirs.enabled
            JOIN organizations workspace ON workspace.id = mine.organization_id
            WHERE mine.actor_id = $1 AND mine.enabled
              AND workspace.lifecycle <> 'deleting'
        )
        "#,
    )
    .bind(one)
    .bind(other)
    .fetch_one(&mut **tx)
    .await?)
}

/// An account's DM setting: `anyone` unless it chose `ask`.
pub(crate) async fn dm_policy(
    tx: &mut Transaction<'_, Postgres>,
    account: Uuid,
) -> Result<String, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT COALESCE((SELECT dm_policy FROM account_profiles WHERE account_id = $1), 'anyone')",
    )
    .bind(account)
    .fetch_one(&mut **tx)
    .await?)
}

/// Checks a message `sender` is about to send into a conversation of
/// `kind`, inside the send's transaction (the conversation is locked). The
/// first message to someone who asks first opens a request; a reply from
/// the person asked accepts it.
pub(crate) async fn check_send(
    tx: &mut Transaction<'_, Postgres>,
    conversation: Uuid,
    kind: &str,
    sender: Uuid,
) -> Result<(), AppError> {
    if kind != "dm" {
        return Ok(());
    }
    let Some(other) = other_member(tx, conversation, sender).await? else {
        return Ok(());
    };
    if blocked_either_way(tx, sender, other).await? {
        return Err(CLOSED);
    }
    if !share_a_workspace(tx, sender, other).await? {
        return Err(NOT_SHARED);
    }
    let request = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT requester_id, state FROM dm_requests WHERE conversation_id = $1 FOR UPDATE",
    )
    .bind(conversation)
    .fetch_optional(&mut **tx)
    .await?;
    match request {
        Some((_, state)) if state == "accepted" => Ok(()),
        Some((requester, _)) if requester == sender => Err(WAITING),
        // The person asked writes back: that accepts it.
        Some(_) => {
            answer(tx, conversation, sender, true).await?;
            Ok(())
        }
        None => {
            if dm_policy(tx, other).await? != "ask" {
                return Ok(());
            }
            // A DM they have already written in is no stranger's.
            let answered = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM messages \
                 WHERE conversation_id = $1 AND author_actor_id = $2 \
                   AND conversation_visible_to($1, $2))",
            )
            .bind(conversation)
            .bind(other)
            .fetch_one(&mut **tx)
            .await?;
            if !answered {
                sqlx::query(
                    "INSERT INTO dm_requests (conversation_id, requester_id, recipient_id) \
                     VALUES ($1, $2, $3)",
                )
                .bind(conversation)
                .bind(sender)
                .bind(other)
                .execute(&mut **tx)
                .await?;
            }
            Ok(())
        }
    }
}

/// Accepts or declines the request in `conversation` made to `recipient`.
/// Returns false when there is no such request.
pub(crate) async fn answer(
    tx: &mut Transaction<'_, Postgres>,
    conversation: Uuid,
    recipient: Uuid,
    accept: bool,
) -> Result<bool, AppError> {
    let changed = sqlx::query(
        "UPDATE dm_requests SET state = $3, answered_at = now() \
         WHERE conversation_id = $1 AND recipient_id = $2 AND state <> 'accepted'",
    )
    .bind(conversation)
    .bind(recipient)
    .bind(if accept { "accepted" } else { "declined" })
    .execute(&mut **tx)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// Whether `recipient` has blocked `actor`: their notifications don't reach
/// the recipient.
pub(crate) async fn has_blocked(
    tx: &mut Transaction<'_, Postgres>,
    recipient: Uuid,
    actor: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM account_blocks \
         WHERE account_id = $1 AND blocked_account_id = $2)",
    )
    .bind(recipient)
    .bind(actor)
    .fetch_one(&mut **tx)
    .await?)
}

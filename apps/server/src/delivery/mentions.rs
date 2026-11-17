//! `comment.mentioned` (C1.1): a task or page comment that names an
//! account tells it, addressed to it alone, the way `message.mentioned`
//! does for messages. The event carries ids (the thing, the comment and the
//! thread it is in), never the comment's words; whether the account may see
//! the thing is checked again whenever the event is read.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    events::{Audience, NewEvent, catalog},
};

/// A comment on a task or page.
pub struct MentioningComment {
    pub space_id: Uuid,
    /// `task` or `page`.
    pub thing_kind: &'static str,
    pub thing_id: Uuid,
    /// `work_item_comment` or `page_comment`.
    pub comment_kind: &'static str,
    pub comment_id: Uuid,
    /// The comment's thread: the comment itself, or a page comment's
    /// thread.
    pub thread_id: Uuid,
    pub author_account_id: Uuid,
}

/// Tells each account the comment mentions, except its author and accounts
/// that aren't active or can't see the thing now.
pub async fn comment_mentioned(
    transaction: &mut Transaction<'_, Postgres>,
    comment: MentioningComment,
    mentioned_account_ids: &[Uuid],
) -> Result<(), AppError> {
    if mentioned_account_ids.is_empty() {
        return Ok(());
    }
    let mentioned = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT actor.id
        FROM actors actor
        WHERE actor.id = ANY($1)
          AND actor.id <> $2
          AND actor.status = 'active'
          AND subject_visible_to($3, $4, actor.id)
        ORDER BY actor.id
        "#,
    )
    .bind(mentioned_account_ids)
    .bind(comment.author_account_id)
    .bind(comment.thing_kind)
    .bind(comment.thing_id)
    .fetch_all(&mut **transaction)
    .await?;
    for account in mentioned {
        crate::events::emit(
            transaction,
            NewEvent::thing(
                &catalog::COMMENT_MENTIONED,
                comment.space_id,
                comment.thing_kind,
                comment.thing_id,
            )
            .access_thing(comment.thing_id)
            .audience(Audience::Account(account))
            .actor(comment.author_account_id)
            .payload(serde_json::json!({
                "commentKind": comment.comment_kind,
                "commentId": comment.comment_id,
                "threadId": comment.thread_id,
            })),
        )
        .await?;
    }
    Ok(())
}

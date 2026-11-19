//! Discussion on any thing (C1.3).
//!
//! A thing's comments are a conversation of kind `comments` whose subject is
//! the thing, one per thing, made by the first comment; anchored discussions
//! are threads under it with an anchor (`anchors`). Comments are messages:
//! [`post`] opens the conversation (and a new anchored thread) in a short
//! first transaction, then sends through the one send pipeline
//! (`append_message_command_with`), so mentions, reply requests, files,
//! previews, notifications and wake-ups are the pipeline's. Who may see a
//! comment is who may see its thing: `conversation_visible_to`'s `subject`
//! arm calls `subject_visible_to`.

pub mod anchors;
pub mod ops;
pub mod subjects;

use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    AppState,
    conversations::{
        AppendExtras, MessageCommandActor, append_message_command_with,
        threads::{self, ThreadCreator, ThreadName, ThreadSpec},
    },
    error::AppError,
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
};
pub use subjects::SubjectKind;

/// Where in a thing's discussion a comment goes.
#[derive(Clone, Debug)]
pub enum CommentPlace {
    /// The thing's comment stream.
    Stream,
    /// A thread already under the thing's comments.
    Thread(Uuid),
    /// A new thread, anchored to a place in the thing or not.
    NewThread { anchor: Option<Value> },
}

/// What a comment says: text (with `@handles`) or composer content.
#[derive(Clone, Debug)]
pub enum CommentBody {
    Text(String),
    Content(Value),
}

/// The comment posted.
#[derive(Clone, Debug)]
pub struct PostedComment {
    pub message_id: Uuid,
    /// The thing's comments conversation.
    pub conversation_id: Uuid,
    pub thread_id: Option<Uuid>,
    /// Where the thing opens.
    pub href: Option<String>,
    pub replayed: bool,
}

/// Where a comments conversation's thing opens (or a thread's in one), for
/// links; `None` for any other conversation. Ids only.
pub(crate) async fn href_of_conversation(
    connection: &mut PgConnection,
    conversation_id: Uuid,
) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        r#"
        -- visibility: the caller holds the conversation, checked
        SELECT subject_href(COALESCE(parent.subject_kind, conversation.subject_kind),
                            COALESCE(parent.subject_id, conversation.subject_id))
        FROM conversations conversation
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = $1
        "#,
    )
    .bind(conversation_id)
    .fetch_optional(&mut *connection)
    .await?
    .flatten())
}

/// Refuses a message into a thing's comments, or a thread in them, once the
/// thing is archived or `sender` can no longer see it, whichever edge sent
/// it. Any other conversation passes.
pub(crate) async fn ensure_subject_takes_messages(
    connection: &mut PgConnection,
    conversation_id: Uuid,
    sender: Uuid,
) -> Result<(), AppError> {
    let subject = subject_of_conversation(connection, conversation_id).await?;
    let Some((kind, thing_id)) = subject else {
        return Ok(());
    };
    let Some(kind) = SubjectKind::parse(&kind) else {
        return Ok(());
    };
    let home = kind
        .subject()
        .home(connection, thing_id, sender)
        .await?
        .ok_or(AppError::NotFound(kind.as_str()))?;
    if !home.accepts_comments {
        return Err(AppError::Conflict(kind.as_str(), "archived"));
    }
    Ok(())
}

/// The thing a comments conversation (or a thread in one) is about: its
/// kind and id; `None` for any other conversation. Ids only.
pub(crate) async fn subject_of_conversation(
    connection: &mut PgConnection,
    conversation_id: Uuid,
) -> Result<Option<(String, Uuid)>, AppError> {
    Ok(sqlx::query_as::<_, (String, Uuid)>(
        r#"
        -- visibility: the caller wrote to or holds the conversation, checked
        SELECT COALESCE(parent.subject_kind, conversation.subject_kind),
               COALESCE(parent.subject_id, conversation.subject_id)
        FROM conversations conversation
        LEFT JOIN conversations parent
          ON parent.id = conversation.parent_channel_id AND parent.kind = 'comments'
        WHERE conversation.id = $1
          AND COALESCE(parent.subject_id, conversation.subject_id) IS NOT NULL
          AND (conversation.kind = 'comments' OR parent.id IS NOT NULL)
        "#,
    )
    .bind(conversation_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The kind of thing a conversation discusses: its comments conversation,
/// or a thread under one. `None` for any other conversation.
pub(crate) async fn subject_kind_of(
    connection: &mut PgConnection,
    conversation_id: Uuid,
) -> Result<Option<SubjectKind>, AppError> {
    Ok(subject_of_conversation(connection, conversation_id)
        .await?
        .and_then(|(kind, _)| SubjectKind::parse(&kind)))
}

/// The thing's comments conversation and its project, made if it doesn't
/// exist yet, when `creator` may see the thing and it takes comments. A
/// board outside any project keeps its comments in the workspace space,
/// which every workspace member belongs to.
pub(crate) async fn open_for(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    creator: &ThreadCreator,
    kind: SubjectKind,
    thing_id: Uuid,
) -> Result<(Uuid, Uuid), AppError> {
    let subject = kind.subject();
    let home = subject
        .home(transaction, thing_id, creator.actor_id)
        .await?
        .ok_or(AppError::NotFound(kind.as_str()))?;
    if !home.accepts_comments {
        return Err(AppError::Conflict(kind.as_str(), "archived"));
    }
    let project_id = match home.project_id {
        Some(project_id) => project_id,
        None => crate::spaces::workspace::join(transaction, home.workspace_id, creator.actor_id)
            .await?
            .ok_or(AppError::NotFound(kind.as_str()))?,
    };
    let (conversation_id, _) =
        threads::open_comments(transaction, creator, project_id, kind.as_str(), thing_id).await?;
    Ok((conversation_id, project_id))
}

/// Moves a thing's discussion to the space the thing moved to: its comments
/// conversation, the threads under it and everything on their messages
/// (one update; the composite keys carry it down, migration
/// `20261117194000_discussions_follow_their_thing`). Called in the
/// transaction that moves the thing, after the thing's own row. Nothing to
/// do for a thing nobody has commented on.
pub(crate) async fn follow(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kind: SubjectKind,
    thing_id: Uuid,
    project_id: Uuid,
) -> Result<(), AppError> {
    hand_over(transaction, kind, thing_id, thing_id, project_id)
        .await
        .map(|_| ())
}

/// Gives a thing's discussion to another thing of the same kind in
/// `project_id` (a task moved to another space is a new task there, C1.4),
/// moving it as [`follow`] does. Returns the comments conversation, if
/// there was one. The new thing has no discussion yet.
pub(crate) async fn hand_over(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kind: SubjectKind,
    from: Uuid,
    to: Uuid,
    project_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let Some(comments_id) = threads::comments_of(transaction, kind.as_str(), from).await? else {
        return Ok(None);
    };
    sqlx::query("SELECT set_config('aw.discussion_move', 'on', true)")
        .execute(&mut **transaction)
        .await?;
    // Where someone last was in the old space isn't a place in the new one.
    sqlx::query(
        r#"
        DELETE FROM project_navigation_states
        WHERE conversation_id = $1
           OR conversation_id IN (
               -- visibility: the tree of a thing's comments, moved with the thing
               SELECT id FROM conversations WHERE parent_channel_id = $1)
        "#,
    )
    .bind(comments_id)
    .execute(&mut **transaction)
    .await?;
    let moved = sqlx::query(
        r#"
        UPDATE conversations SET project_id = $3, subject_id = $2
        WHERE id = $1 AND (project_id <> $3 OR subject_id <> $2)
        "#,
    )
    .bind(comments_id)
    .bind(to)
    .bind(project_id)
    .execute(&mut **transaction)
    .await;
    sqlx::query("SELECT set_config('aw.discussion_move', 'off', true)")
        .execute(&mut **transaction)
        .await?;
    match moved {
        Ok(_) => Ok(Some(comments_id)),
        // Something tied to the old space hangs off the discussion (an
        // agent's run that can't move with it).
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("23503") => {
            Err(AppError::Conflict(
                kind.as_str(),
                "its discussion has work tied to its current space",
            ))
        }
        Err(error) => Err(error.into()),
    }
}

/// A message a thing is made from (C2.22): one the actor can read, in a
/// conversation of `space`. Its id, conversation and text, or None.
pub(crate) async fn message_in_space(
    connection: &mut sqlx::PgConnection,
    message_id: Uuid,
    space: Uuid,
    actor: Uuid,
) -> Result<Option<(Uuid, Uuid, String)>, AppError> {
    Ok(sqlx::query_as::<_, (Uuid, Uuid, String)>(
        "SELECT message.id, message.conversation_id, message.plain_text \
         FROM messages message \
         WHERE message.id = $1 AND message.project_id = $2 \
           AND message.removed_at IS NULL \
           AND conversation_visible_to(message.conversation_id, $3)",
    )
    .bind(message_id)
    .bind(space)
    .bind(actor)
    .fetch_optional(&mut *connection)
    .await?)
}

/// Posts a comment on a thing.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn post(
    state: &AppState,
    actor: MessageCommandActor,
    creator: ThreadCreator,
    kind: SubjectKind,
    thing_id: Uuid,
    place: CommentPlace,
    key: IdempotencyKey,
    body: CommentBody,
    attachments: &[Uuid],
    reply_to_message_id: Option<Uuid>,
) -> Result<PostedComment, AppError> {
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let (comments_id, project_id) = open_for(&mut transaction, &creator, kind, thing_id).await?;
    // A task's discussion only while the author, and an app acting for
    // them, still sees tasks there.
    if crate::conversations::task_discussion_hidden(&mut transaction, creator.actor_id, comments_id)
        .await?
    {
        return Err(AppError::NotFound("task"));
    }
    let (conversation_id, expected_kind, thread_id) = match place {
        CommentPlace::Stream => (comments_id, "comments", None),
        CommentPlace::Thread(thread_id) => {
            let under = sqlx::query_scalar::<_, bool>(
                r#"
                -- visibility: a thread under the comments conversation checked above
                SELECT EXISTS (
                    SELECT 1 FROM conversations
                    WHERE id = $1 AND parent_channel_id = $2 AND kind = 'thread'
                )
                "#,
            )
            .bind(thread_id)
            .bind(comments_id)
            .fetch_one(&mut *transaction)
            .await?;
            if !under {
                return Err(AppError::NotFound("thread"));
            }
            (thread_id, "thread", Some(thread_id))
        }
        CommentPlace::NewThread { anchor } => {
            let anchor = match anchor {
                Some(anchor) => {
                    Some(anchors::check(&mut transaction, kind.subject(), thing_id, &anchor).await?)
                }
                None => None,
            };
            // A retry with the same request id finds the thread it made.
            let scope =
                idempotency::resource_bound_scope("conversation", comments_id, "comment_thread")?;
            let thread_id = match idempotency::reserve(
                &mut transaction,
                creator.actor_id,
                &scope,
                &key,
                RequestHash::for_value(&json!({ "anchor": anchor }))?,
            )
            .await?
            {
                IdempotencyOutcome::Replay(replay) => replay.decode::<Uuid>()?,
                IdempotencyOutcome::Reserved(reservation) => {
                    let thread = threads::create(
                        &mut transaction,
                        &creator,
                        comments_id,
                        ThreadSpec::new(ThreadName::Derived).anchored(anchor),
                    )
                    .await?;
                    idempotency::complete(
                        &mut transaction,
                        &reservation,
                        axum::http::StatusCode::CREATED,
                        &thread.id,
                    )
                    .await?;
                    thread.id
                }
            };
            (thread_id, "thread", Some(thread_id))
        }
    };
    // Files uploaded to the thing's discussion go with a comment posted in
    // a thread under it.
    if conversation_id != comments_id {
        crate::files::rehome_thread_uploads(
            &mut transaction,
            comments_id,
            conversation_id,
            actor.actor_id,
            attachments,
        )
        .await?;
    }
    // A task's comment files were uploaded to the task (`?for=comment`);
    // they go with the comment's conversation now.
    if kind == SubjectKind::Task {
        crate::files::rehome_comment_uploads(
            &mut transaction,
            thing_id,
            conversation_id,
            actor.actor_id,
            attachments,
        )
        .await?;
    }
    let href = href_of_conversation(&mut transaction, comments_id).await?;
    transaction.commit().await?;

    let content = match body {
        CommentBody::Content(content) => content,
        CommentBody::Text(text) => {
            crate::mcp::content_from_text(state, project_id, None, &text).await?
        }
    };
    let outcome = append_message_command_with(
        state,
        actor,
        conversation_id,
        Some(expected_kind),
        key,
        content,
        attachments,
        None,
        AppendExtras {
            reply_to_message_id,
            ..AppendExtras::default()
        },
    )
    .await?;
    Ok(PostedComment {
        message_id: outcome.body.message.id,
        conversation_id: comments_id,
        thread_id,
        href,
        replayed: outcome.idempotent_replay,
    })
}

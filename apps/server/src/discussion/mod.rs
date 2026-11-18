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
    if !subject.open {
        return Err(AppError::Refused {
            field: "thing",
            message: "Comment on a task with task_comment and on a page with page_comment; \
                      they move to comment_create later.",
        });
    }
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
    let href = href_of_conversation(&mut transaction, comments_id).await?;
    transaction.commit().await?;

    let content = match body {
        CommentBody::Content(content) => content,
        CommentBody::Text(text) => crate::mcp::content_from_text(state, project_id, &text).await?,
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

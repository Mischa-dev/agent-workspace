//! Task comments (C1.4): messages in the task's comments conversation
//! (C1.3), posted through the one send pipeline (`discussion::post`) and
//! edited and removed through the message commands (`message_edits`). The
//! task's timeline lists each one in the order it came (an entry a trigger
//! on messages writes, migration `20261117210100_comments_are_conversations`).
//! A comment's id is its message's.
//!
//! Its author edits a comment; the author, or someone whose role moderates
//! the conversation (a person), removes it. The routes and tools keep the
//! shapes they had before comments were messages.

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, patch},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{
    WorkItemCommentView, authorize_task_comment, authorize_work_item, message_content_error,
    project_membership, timeline_entry_view,
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{ActorView, MessageCommandActor, threads::ThreadCreator},
    discussion::{CommentBody, CommentPlace, SubjectKind},
    error::{ApiJson, ApiPath, AppError},
    idempotency::IdempotencyKey,
    message_content::MessageContent,
    message_edits::{self, Change, ChangeError, Refused},
    policy::Action,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/comments/{comment_id}",
            patch(edit_comment_http).delete(remove_comment_http),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/comments/{comment_id}/revisions",
            get(comment_revisions_http),
        )
}

/// Why a change wasn't made. Each surface words these its own way.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommentRefusal {
    /// Editing is for the comment's author only.
    NotAuthor,
    /// Removing someone else's comment takes a role that moderates here.
    NotModerator,
    Removed,
    /// The comment changed since it was read; carries the current revision.
    RevisionMoved(i32),
    TooManyEdits,
    TaskArchived,
}

pub(crate) struct CommentEdit {
    pub(crate) changed: bool,
    pub(crate) revision: i32,
}

/// The comment posted, and its place in the task's timeline.
pub(crate) struct PostedTaskComment {
    pub(crate) message_id: Uuid,
    pub(crate) sequence: i64,
    pub(crate) replayed: bool,
}

/// Posts a comment on a task, for the web and agent tools alike. The
/// caller has checked the poster may change the task. `reply_to` is a
/// comment on the same task that isn't removed (else
/// `Validation("replyToCommentId")`); `attachments` are the poster's
/// unsent uploads for a comment on this task.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn post(
    state: &AppState,
    actor: MessageCommandActor,
    creator: ThreadCreator,
    work_item_id: Uuid,
    content: serde_json::Value,
    attachments: &[Uuid],
    reply_to: Option<Uuid>,
    key: IdempotencyKey,
    audit: (&crate::work_item_commands::CommandActor, &'static str),
) -> Result<PostedTaskComment, AppError> {
    // The send pipeline audits a message sent with an MCP credential itself;
    // every other poster (a browser session, a connected app's grant) gets
    // the task's audit row here.
    let audited_by_pipeline = actor.mcp_credential.is_some();
    let posted = crate::discussion::post(
        state,
        actor,
        creator,
        SubjectKind::Task,
        work_item_id,
        CommentPlace::Stream,
        key,
        CommentBody::Content(content),
        attachments,
        reply_to,
    )
    .await
    .map_err(|error| match error {
        AppError::Refused {
            field: "replyToMessageId",
            ..
        } => AppError::Validation("replyToCommentId"),
        other => other,
    })?;
    let sequence = sqlx::query_scalar::<_, i64>(
        "SELECT sequence FROM work_item_timeline_entries WHERE work_item_id = $1 AND message_id = $2",
    )
    .bind(work_item_id)
    .bind(posted.message_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?
    .ok_or(AppError::Internal("task comment timeline entry"))?;
    if !posted.replayed {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        task_changed(&mut transaction, work_item_id).await?;
        // The audit row names the comment by id only, with the request id
        // it came with.
        if !audited_by_pipeline {
            crate::work_item_commands::record_audit(
                &mut transaction,
                audit.0,
                audit.1,
                work_item_id,
                serde_json::json!({ "commentId": posted.message_id }),
            )
            .await?;
        }
        transaction.commit().await?;
    }
    Ok(PostedTaskComment {
        message_id: posted.message_id,
        sequence,
        replayed: posted.replayed,
    })
}

/// Tells open boards and task pages that the task's timeline changed, as
/// any change to the task does (`work_item.updated`).
async fn task_changed(
    transaction: &mut Transaction<'_, Postgres>,
    work_item_id: Uuid,
) -> Result<(), AppError> {
    let Some((project_id, version)) = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT project_id, version FROM work_items WHERE id = $1",
    )
    .bind(work_item_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(());
    };
    crate::work_item_commands::enqueue_update(transaction, project_id, work_item_id, version).await
}

/// The task's comments conversation, and whether the task is archived.
/// NotFound("comment") when the task has no comments.
async fn task_comments(
    transaction: &mut Transaction<'_, Postgres>,
    work_item_id: Uuid,
) -> Result<(Uuid, bool), AppError> {
    let archived = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NOT NULL FROM task_rows WHERE id = $1",
    )
    .bind(work_item_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("comment"))?;
    let comments = crate::conversations::threads::comments_of(transaction, "task", work_item_id)
        .await?
        .ok_or(AppError::NotFound("comment"))?;
    Ok((comments, archived))
}

fn refusal(error: ChangeError, change: Change) -> Result<CommentRefusal, AppError> {
    match error {
        ChangeError::App(error) => Err(error),
        ChangeError::Refused(refused) => Ok(match refused {
            Refused::NotYourMessage if change == Change::Remove => CommentRefusal::NotModerator,
            Refused::NotYourMessage => CommentRefusal::NotAuthor,
            Refused::RoleCannotWrite => return Err(AppError::Forbidden),
            Refused::Removed => CommentRefusal::Removed,
            Refused::Archived => CommentRefusal::TaskArchived,
            Refused::RevisionMoved(current) => CommentRefusal::RevisionMoved(current),
            Refused::TooManyEdits => CommentRefusal::TooManyEdits,
        }),
    }
}

/// Edits a comment on a task that isn't archived: the author only. The
/// same text again changes nothing. Earlier versions stay in the message's
/// revisions; mentions and references follow the new text.
pub(crate) async fn edit(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MessageCommandActor,
    work_item_id: Uuid,
    comment_id: Uuid,
    content: MessageContent,
    expected_revision: Option<i32>,
) -> Result<Result<CommentEdit, CommentRefusal>, AppError> {
    let (comments, archived) = task_comments(transaction, work_item_id).await?;
    let locked = match message_edits::lock_message_for_change(
        transaction,
        actor,
        Some(comments),
        comment_id,
        Change::Edit,
    )
    .await
    {
        Ok(locked) => locked,
        Err(ChangeError::App(AppError::NotFound(_))) => return Err(AppError::NotFound("comment")),
        Err(error) => return refusal(error, Change::Edit).map(Err),
    };
    if locked.removed {
        return Ok(Err(CommentRefusal::Removed));
    }
    if archived {
        return Ok(Err(CommentRefusal::TaskArchived));
    }
    match message_edits::apply_edit(transaction, &locked, actor, content, expected_revision).await {
        Ok(edit) => {
            if edit.changed {
                task_changed(transaction, work_item_id).await?;
            }
            Ok(Ok(CommentEdit {
                changed: edit.changed,
                revision: edit
                    .entry
                    .message
                    .as_ref()
                    .map_or(locked.revision, |message| message.revision),
            }))
        }
        Err(error) => refusal(error, Change::Edit).map(Err),
    }
}

/// Removes a comment for everyone: its text, every version's text, its
/// mentions, references and files go, and its place in the task's history
/// says who removed it. Returns whether it was already removed.
pub(crate) async fn remove(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MessageCommandActor,
    work_item_id: Uuid,
    comment_id: Uuid,
) -> Result<Result<bool, CommentRefusal>, AppError> {
    let (comments, _) = task_comments(transaction, work_item_id).await?;
    let locked = match message_edits::lock_message_for_change(
        transaction,
        actor,
        Some(comments),
        comment_id,
        Change::Remove,
    )
    .await
    {
        Ok(locked) => locked,
        Err(ChangeError::App(AppError::NotFound(_))) => return Err(AppError::NotFound("comment")),
        Err(error) => return refusal(error, Change::Remove).map(Err),
    };
    match message_edits::apply_removal(transaction, &locked, actor).await {
        Ok(removal) => {
            if !removal.already_removed {
                task_changed(transaction, work_item_id).await?;
            }
            Ok(Ok(removal.already_removed))
        }
        Err(error) => refusal(error, Change::Remove).map(Err),
    }
}

/// One version of a comment, oldest first in a history.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommentRevisionView {
    pub(crate) revision: i32,
    /// Null once the comment was deleted: every version's text is gone.
    pub(crate) content: Option<serde_json::Value>,
    pub(crate) plain_text: Option<String>,
    pub(crate) written_by: ActorView,
    pub(crate) written_at: String,
    pub(crate) redacted: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommentRevisionsView {
    pub(crate) comment_id: Uuid,
    /// The current revision.
    pub(crate) revision: i32,
    pub(crate) removed: bool,
    /// Oldest first. Empty for a comment never edited.
    pub(crate) revisions: Vec<CommentRevisionView>,
}

#[derive(FromRow)]
struct RevisionRow {
    revision: i32,
    content: Option<serde_json::Value>,
    plain_text: Option<String>,
    actor_id: Uuid,
    actor_kind: String,
    display_name: String,
    handle: String,
    written_at: String,
    redacted: bool,
}

/// A comment's versions. The caller has checked the reader may see the task.
pub(crate) async fn comment_revisions(
    pool: &PgPool,
    project_id: Uuid,
    work_item_id: Uuid,
    comment_id: Uuid,
) -> Result<Option<CommentRevisionsView>, AppError> {
    let Some((revision, removed)) = sqlx::query_as::<_, (i32, bool)>(
        r#"
        -- visibility: a comment of a task the reader may see, which is the rule
        SELECT message.revision, message.removed_at IS NOT NULL
        FROM messages message
        JOIN conversations place ON place.id = message.conversation_id
        WHERE message.id = $3 AND message.project_id = $1
          AND place.kind = 'comments' AND place.subject_kind = 'task'
          AND place.subject_id = $2
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(comment_id)
    .fetch_optional(crate::db::scoped(pool))
    .await?
    else {
        return Ok(None);
    };
    let revisions = sqlx::query_as::<_, RevisionRow>(
        r#"
        SELECT revision.revision, revision.content, revision.plain_text,
               actor.id AS actor_id, actor.kind AS actor_kind,
               membership.display_name, membership.handle,
               revision.written_at::text AS written_at,
               revision.redacted_at IS NOT NULL AS redacted
        FROM message_revisions revision
        JOIN actors actor ON actor.id = revision.written_by_actor_id
        JOIN project_memberships membership
          ON membership.project_id = revision.project_id
         AND membership.actor_id = revision.written_by_actor_id
        WHERE revision.project_id = $1 AND revision.message_id = $2
        ORDER BY revision.revision
        "#,
    )
    .bind(project_id)
    .bind(comment_id)
    .fetch_all(crate::db::scoped(pool))
    .await?
    .into_iter()
    .map(|row| CommentRevisionView {
        revision: row.revision,
        content: row.content,
        plain_text: row.plain_text,
        written_by: ActorView {
            id: row.actor_id,
            kind: row.actor_kind,
            display_name: row.display_name,
            handle: row.handle,
            avatar_url: None,
        },
        written_at: row.written_at,
        redacted: row.redacted,
    })
    .collect();
    Ok(Some(CommentRevisionsView {
        comment_id,
        revision,
        removed,
        revisions,
    }))
}

/// The comment as the task's timeline shows it now, to `viewer`: the tasks
/// it references without their titles and statuses unless they may see
/// tasks here (commenting takes only "edit tasks").
pub(super) async fn comment_view(
    pool: &PgPool,
    viewer: Uuid,
    project_id: Uuid,
    work_item_id: Uuid,
    comment_id: Uuid,
) -> Result<WorkItemCommentView, AppError> {
    let row = super::fetch_comment_timeline_row(pool, project_id, work_item_id, comment_id)
        .await?
        .ok_or(AppError::NotFound("comment"))?;
    let mut comment = timeline_entry_view(pool, project_id, work_item_id, row)
        .await?
        .comment
        .ok_or(AppError::Internal("comment view"))?;
    if !crate::conversations::sees_tasks_in(pool, viewer, project_id).await? {
        super::hide_reference_details(&mut comment);
    }
    Ok(comment)
}

// ---------------------------------------------------------------------------
// HTTP

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EditCommentRequest {
    content: serde_json::Value,
    /// The revision the editor started from; a newer one is a conflict.
    expected_revision: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditCommentResponse {
    comment: WorkItemCommentView,
    /// False when the comment already said exactly this.
    changed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoveCommentResponse {
    comment: WorkItemCommentView,
    already_removed: bool,
}

fn http_refusal(refusal: CommentRefusal) -> AppError {
    match refusal {
        CommentRefusal::NotAuthor | CommentRefusal::NotModerator => AppError::Forbidden,
        CommentRefusal::Removed => AppError::Conflict("comment", "removed"),
        CommentRefusal::RevisionMoved(current) => AppError::VersionConflict {
            resource: "comment",
            current_version: i64::from(current),
        },
        CommentRefusal::TooManyEdits => AppError::Conflict("comment", "too_many_edits"),
        CommentRefusal::TaskArchived => AppError::Conflict("work_item", "archived"),
    }
}

pub(crate) fn browser_message_actor(principal: &AuthenticatedActor) -> MessageCommandActor {
    MessageCommandActor {
        actor_id: principal.actor_id,
        actor_kind: principal.actor_kind.clone(),
        mcp_credential: None,
        via_grant_id: None,
        run_id: None,
    }
}

/// Checks the caller may comment on the task now ("comment on tasks",
/// project not archived). An archived task's comments can still be removed.
async fn check_commenter(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<(), AppError> {
    match authorize_task_comment(transaction, principal, project_id, work_item_id).await {
        Ok(()) | Err(AppError::Conflict("work_item", "archived")) => Ok(()),
        Err(error) => Err(error),
    }
}

async fn edit_comment_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, comment_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<EditCommentRequest>,
) -> Result<Json<EditCommentResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let content = MessageContent::from_json(request.content).map_err(message_content_error)?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    check_commenter(&mut transaction, &principal, project_id, work_item_id).await?;
    let edited = edit(
        &mut transaction,
        &browser_message_actor(&principal),
        work_item_id,
        comment_id,
        content,
        request.expected_revision,
    )
    .await?
    .map_err(http_refusal)?;
    transaction.commit().await?;
    Ok(Json(EditCommentResponse {
        comment: comment_view(
            &state.pool,
            principal.actor_id,
            project_id,
            work_item_id,
            comment_id,
        )
        .await?,
        changed: edited.changed,
    }))
}

async fn remove_comment_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, comment_id)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> Result<Json<RemoveCommentResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    check_commenter(&mut transaction, &principal, project_id, work_item_id).await?;
    let already_removed = remove(
        &mut transaction,
        &browser_message_actor(&principal),
        work_item_id,
        comment_id,
    )
    .await?
    .map_err(http_refusal)?;
    transaction.commit().await?;
    Ok(Json(RemoveCommentResponse {
        comment: comment_view(
            &state.pool,
            principal.actor_id,
            project_id,
            work_item_id,
            comment_id,
        )
        .await?,
        already_removed,
    }))
}

async fn comment_revisions_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, comment_id)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> Result<Json<CommentRevisionsView>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemView,
        Some(work_item_id),
    )?;
    Ok(Json(
        comment_revisions(&state.pool, project_id, work_item_id, comment_id)
            .await?
            .ok_or(AppError::NotFound("comment"))?,
    ))
}

/// For the routes that post a comment: who posts, as the pipeline and the
/// discussion's audit rows name them.
pub(crate) fn browser_poster(
    principal: &AuthenticatedActor,
    headers: &HeaderMap,
) -> Result<(MessageCommandActor, ThreadCreator), AppError> {
    Ok((
        browser_message_actor(principal),
        ThreadCreator::browser(principal, headers)?,
    ))
}

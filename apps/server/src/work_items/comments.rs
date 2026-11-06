//! Editing and deleting task comments, for the web and MCP alike. It mirrors
//! message edits (`crate::message_edits`, migration 0047): each command sets
//! `aw.comment_command` to the one comment it changes, for its transaction
//! only. An edit keeps the old and new versions in
//! `work_item_comment_revisions`; a deletion blanks the comment and every
//! revision but keeps its place in the task's history, with who deleted it.
//!
//! Only the author edits a comment. The author, or a project owner or admin
//! (a person), deletes it.

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
    WorkItemCommentView, authorize_work_item, browser_actor, fetch_comment_actor_rows,
    fetch_work_item_reference_rows, lock_project_membership, message_content_error,
    project_membership, timeline_entry_view,
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::ActorView,
    error::{ApiJson, ApiPath, AppError},
    message_content::{MessageContent, MessageNode},
    policy::{Action, ActorKind, Permission},
    work_item_commands::{CommandActor, enqueue_update, record_audit},
};

/// A comment takes at most this many versions, as messages do.
pub(crate) const MAX_COMMENT_REVISION: i32 = 1000;
const MAX_COMMENT_CHARACTERS: usize = 20_000;

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

/// A comment locked for a change.
#[derive(Debug, FromRow)]
pub(crate) struct LockedComment {
    pub(crate) id: Uuid,
    pub(crate) project_id: Uuid,
    pub(crate) work_item_id: Uuid,
    pub(crate) author_actor_id: Uuid,
    pub(crate) revision: i32,
    pub(crate) content: serde_json::Value,
    pub(crate) removed: bool,
    pub(crate) task_archived: bool,
}

/// Why a change wasn't made. Each surface words these its own way.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommentRefusal {
    /// Editing is for the comment's author only.
    NotAuthor,
    /// Deleting someone else's comment takes a project owner or admin.
    NotModerator,
    Removed,
    /// The comment changed since it was read; carries the current revision.
    RevisionMoved(i32),
    TooManyEdits,
    TaskArchived,
}

/// Content a comment can be saved with: its mentions and task references
/// checked against the project now.
pub(crate) struct ResolvedComment {
    pub(crate) content_json: serde_json::Value,
    pub(crate) plain_text: String,
    pub(crate) mention_nodes: Vec<(usize, Uuid)>,
    pub(crate) references: Vec<(usize, Uuid)>,
}

pub(crate) struct CommentEdit {
    pub(crate) changed: bool,
    pub(crate) revision: i32,
}

/// Check `content` the way a new comment's is checked: people it mentions
/// are enabled members, tasks it references are in the project.
pub(crate) async fn resolve_comment_content(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    content: MessageContent,
) -> Result<ResolvedComment, AppError> {
    let mention_ids = content.mentioned_actor_ids();
    let mention_rows = fetch_comment_actor_rows(transaction, project_id, &mention_ids).await?;
    let handles = mention_rows
        .iter()
        .map(|actor| (actor.id, actor.handle.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    let references = content.work_item_references();
    let reference_ids = references
        .iter()
        .map(|(_, work_item_id)| *work_item_id)
        .collect::<Vec<_>>();
    let reference_rows =
        fetch_work_item_reference_rows(transaction, project_id, &reference_ids).await?;
    let labels = reference_rows
        .iter()
        .map(|item| (item.id, item.key.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    let mention_nodes = content
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| match node {
            MessageNode::Mention { actor_id } => Some((index, *actor_id)),
            MessageNode::Text { .. } | MessageNode::WorkItemRef { .. } => None,
        })
        .collect::<Vec<_>>();
    let (content, plain_text, _) = content
        .validate_with_resources(
            |actor_id| handles.get(&actor_id).cloned(),
            |referenced_id| labels.get(&referenced_id).cloned(),
        )
        .map_err(message_content_error)?
        .into_parts();
    if plain_text.trim().is_empty() || plain_text.chars().count() > MAX_COMMENT_CHARACTERS {
        return Err(AppError::Validation("comment content"));
    }
    let content_json = serde_json::to_value(content)
        .map_err(|_| AppError::Internal("serialize Work Item comment"))?;
    Ok(ResolvedComment {
        content_json,
        plain_text,
        mention_nodes,
        references,
    })
}

/// Lock one comment (and only it) for an edit or a deletion. None when it
/// isn't on that task in that project.
pub(crate) async fn lock_comment(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    comment_id: Uuid,
) -> Result<Option<LockedComment>, AppError> {
    Ok(sqlx::query_as::<_, LockedComment>(
        r#"
        SELECT comment.id, comment.project_id, comment.work_item_id,
               comment.author_actor_id, comment.revision, comment.content,
               comment.removed_at IS NOT NULL AS removed,
               item.archived_at IS NOT NULL AS task_archived
        FROM work_item_comments comment
        JOIN work_items item
          ON item.project_id = comment.project_id AND item.id = comment.work_item_id
        WHERE comment.project_id = $1 AND comment.work_item_id = $2 AND comment.id = $3
        FOR UPDATE OF comment
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(comment_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn allow_change(
    transaction: &mut Transaction<'_, Postgres>,
    comment_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query("SELECT set_config('aw.comment_command', $1, true)")
        .bind(comment_id.to_string())
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn insert_projections(
    transaction: &mut Transaction<'_, Postgres>,
    locked: &LockedComment,
    resolved: &ResolvedComment,
) -> Result<(), AppError> {
    for (node_index, actor_id) in &resolved.mention_nodes {
        sqlx::query(
            "INSERT INTO work_item_comment_mentions (project_id, work_item_id, comment_id, actor_id, node_index) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(locked.project_id)
        .bind(locked.work_item_id)
        .bind(locked.id)
        .bind(actor_id)
        .bind(i32::try_from(*node_index).map_err(|_| AppError::Validation("comment content"))?)
        .execute(&mut **transaction)
        .await?;
    }
    for (node_index, referenced_id) in &resolved.references {
        sqlx::query(
            "INSERT INTO work_item_comment_references (project_id, source_work_item_id, comment_id, referenced_work_item_id, node_index) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(locked.project_id)
        .bind(locked.work_item_id)
        .bind(locked.id)
        .bind(referenced_id)
        .bind(i32::try_from(*node_index).map_err(|_| AppError::Validation("comment content"))?)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

/// Edit a locked comment: the author only, while the task is open. The
/// first edit keeps the original as revision 1; each edit adds the next.
/// The same text again changes nothing.
pub(crate) async fn apply_comment_edit(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    locked: &LockedComment,
    resolved: ResolvedComment,
    expected_revision: Option<i32>,
) -> Result<Result<CommentEdit, CommentRefusal>, AppError> {
    if locked.removed {
        return Ok(Err(CommentRefusal::Removed));
    }
    if locked.author_actor_id != actor.actor_id {
        return Ok(Err(CommentRefusal::NotAuthor));
    }
    if locked.task_archived {
        return Ok(Err(CommentRefusal::TaskArchived));
    }
    if expected_revision.is_some_and(|expected| expected != locked.revision) {
        return Ok(Err(CommentRefusal::RevisionMoved(locked.revision)));
    }
    if resolved.content_json == locked.content {
        return Ok(Ok(CommentEdit {
            changed: false,
            revision: locked.revision,
        }));
    }
    if locked.revision >= MAX_COMMENT_REVISION {
        return Ok(Err(CommentRefusal::TooManyEdits));
    }
    allow_change(transaction, locked.id).await?;
    if locked.revision == 1 {
        sqlx::query(
            r#"
            INSERT INTO work_item_comment_revisions (
                project_id, work_item_id, comment_id, revision, content, plain_text,
                written_by_actor_id, via_oauth_grant_id, written_at
            )
            SELECT project_id, work_item_id, id, revision, content, plain_text,
                   author_actor_id, via_oauth_grant_id, created_at
            FROM work_item_comments
            WHERE id = $1 AND revision = 1
            ON CONFLICT (comment_id, revision) DO NOTHING
            "#,
        )
        .bind(locked.id)
        .execute(&mut **transaction)
        .await?;
    }
    let next_revision = locked.revision + 1;
    sqlx::query(
        r#"
        INSERT INTO work_item_comment_revisions (
            project_id, work_item_id, comment_id, revision, content, plain_text,
            written_by_actor_id, via_oauth_grant_id, written_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, current_via_grant(), now())
        "#,
    )
    .bind(locked.project_id)
    .bind(locked.work_item_id)
    .bind(locked.id)
    .bind(next_revision)
    .bind(&resolved.content_json)
    .bind(&resolved.plain_text)
    .bind(actor.actor_id)
    .execute(&mut **transaction)
    .await?;
    let updated = sqlx::query(
        r#"
        UPDATE work_item_comments
        SET content = $3, plain_text = $4, revision = revision + 1, edited_at = now()
        WHERE id = $1 AND revision = $2 AND removed_at IS NULL
        "#,
    )
    .bind(locked.id)
    .bind(locked.revision)
    .bind(&resolved.content_json)
    .bind(&resolved.plain_text)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal("edit a locked task comment"));
    }
    // Projections follow the new text: every row again, at its new place.
    sqlx::query("DELETE FROM work_item_comment_mentions WHERE comment_id = $1")
        .bind(locked.id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM work_item_comment_references WHERE comment_id = $1")
        .bind(locked.id)
        .execute(&mut **transaction)
        .await?;
    insert_projections(transaction, locked, &resolved).await?;

    let version = sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
        .bind(locked.work_item_id)
        .fetch_one(&mut **transaction)
        .await?;
    // The update tells open task pages, and the notifier finds the edit by
    // its time (newly mentioned people hear about it once).
    enqueue_update(transaction, locked.project_id, locked.work_item_id, version).await?;
    record_audit(
        transaction,
        actor,
        "work_item.comment_edited",
        locked.work_item_id,
        serde_json::json!({"commentId": locked.id, "revision": next_revision}),
    )
    .await?;
    Ok(Ok(CommentEdit {
        changed: true,
        revision: next_revision,
    }))
}

/// Delete a locked comment for everyone: its text, every version's text,
/// its mentions and task references and its files go, and the comment's
/// place in the task's history says who deleted it. The author may delete
/// their own comment; `moderator` (a project owner or admin, a person) may
/// delete anyone's. Deleting it again changes nothing. Returns whether it
/// was already deleted.
pub(crate) async fn apply_comment_removal(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    moderator: bool,
    locked: &LockedComment,
) -> Result<Result<bool, CommentRefusal>, AppError> {
    let own = locked.author_actor_id == actor.actor_id;
    if !own && !moderator {
        return Ok(Err(CommentRefusal::NotModerator));
    }
    if locked.removed {
        return Ok(Ok(true));
    }
    allow_change(transaction, locked.id).await?;
    sqlx::query(
        r#"
        UPDATE work_item_comment_revisions
        SET content = NULL, plain_text = NULL, redacted_at = now(), redacted_by_actor_id = $2
        WHERE comment_id = $1 AND redacted_at IS NULL
        "#,
    )
    .bind(locked.id)
    .bind(actor.actor_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM work_item_comment_mentions WHERE comment_id = $1")
        .bind(locked.id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM work_item_comment_references WHERE comment_id = $1")
        .bind(locked.id)
        .execute(&mut **transaction)
        .await?;
    crate::files::forget_comment_files(transaction, locked.id, actor.actor_id).await?;
    let updated = sqlx::query(
        r#"
        UPDATE work_item_comments
        SET content = '{"version": 1, "nodes": []}'::jsonb, plain_text = '',
            removed_at = now(), removed_by_actor_id = $2
        WHERE id = $1 AND removed_at IS NULL
        "#,
    )
    .bind(locked.id)
    .bind(actor.actor_id)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal("delete a locked task comment"));
    }
    scrub_create_replays(transaction, locked).await?;
    crate::notifications::forget_comment(transaction, locked.project_id, locked.id).await?;
    let version = sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1")
        .bind(locked.work_item_id)
        .fetch_one(&mut **transaction)
        .await?;
    enqueue_update(transaction, locked.project_id, locked.work_item_id, version).await?;
    record_audit(
        transaction,
        actor,
        "work_item.comment_deleted",
        locked.work_item_id,
        serde_json::json!({"commentId": locked.id, "ownComment": own}),
    )
    .await?;
    Ok(Ok(false))
}

/// The stored replays of the comment's creation hold its text (the web's)
/// or a hash of it (task_comment's): blank the text and swap the hash for
/// one no request can match, so a retry with the old key is refused instead
/// of posting the text again, and a database copy can't confirm a guess.
async fn scrub_create_replays(
    transaction: &mut Transaction<'_, Postgres>,
    locked: &LockedComment,
) -> Result<(), AppError> {
    let tombstone = crate::idempotency::RequestHash::for_value(&MessageContent {
        version: 1,
        nodes: Vec::new(),
    })?;
    sqlx::query(
        r#"
        UPDATE idempotency_keys
        SET request_hash = $3,
            response_body = jsonb_set(
                jsonb_set(
                    jsonb_set(response_body, '{comment,content}', '{"version": 1, "nodes": []}'::jsonb),
                    '{comment,plainText}', '""'::jsonb
                ),
                '{comment,mentionedActors}', '[]'::jsonb
            )
        WHERE scope = $1 AND response_body->'comment'->>'id' = $2
        "#,
    )
    .bind(crate::idempotency::resource_bound_scope(
        "work_items",
        locked.work_item_id,
        "comment_create",
    )?)
    .bind(locked.id.to_string())
    .bind(tombstone.as_bytes().as_slice())
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE idempotency_keys
        SET request_hash = $3
        WHERE scope = $1 AND response_body->>'activityId' = $2
        "#,
    )
    .bind(crate::idempotency::resource_bound_scope(
        "mcp_work_items",
        locked.work_item_id,
        "post_update",
    )?)
    .bind(locked.id.to_string())
    .bind(tombstone.as_bytes().as_slice())
    .execute(&mut **transaction)
    .await?;
    Ok(())
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
        SELECT revision, removed_at IS NOT NULL
        FROM work_item_comments
        WHERE project_id = $1 AND work_item_id = $2 AND id = $3
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(comment_id)
    .fetch_optional(pool)
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
        FROM work_item_comment_revisions revision
        JOIN actors actor ON actor.id = revision.written_by_actor_id
        JOIN project_memberships membership
          ON membership.project_id = revision.project_id
         AND membership.actor_id = revision.written_by_actor_id
        WHERE revision.project_id = $1 AND revision.work_item_id = $2
          AND revision.comment_id = $3
        ORDER BY revision.revision
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(comment_id)
    .fetch_all(pool)
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
async fn comment_view(
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

/// Locks the caller's membership, checks they may comment on the task now
/// (WorkItemEdit, project not archived) and locks the comment.
async fn lock_for_change(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    work_item_id: Uuid,
    comment_id: Uuid,
) -> Result<(LockedComment, bool), AppError> {
    let membership = lock_project_membership(transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let locked = lock_comment(transaction, project_id, work_item_id, comment_id)
        .await?
        .ok_or(AppError::NotFound("comment"))?;
    let moderator =
        principal.actor_kind == "human" && membership.may(Permission::ChannelsModerate)?;
    Ok((locked, moderator))
}

async fn edit_comment_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, comment_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<EditCommentRequest>,
) -> Result<Json<EditCommentResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let content = MessageContent::from_json(request.content).map_err(message_content_error)?;
    let mut transaction = state.pool.begin().await?;
    let (locked, _) = lock_for_change(
        &mut transaction,
        &principal,
        project_id,
        work_item_id,
        comment_id,
    )
    .await?;
    let resolved = resolve_comment_content(&mut transaction, project_id, content).await?;
    let actor = browser_actor(&headers, &principal, project_id)?;
    let edit = apply_comment_edit(
        &mut transaction,
        &actor,
        &locked,
        resolved,
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
        changed: edit.changed,
    }))
}

async fn remove_comment_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, comment_id)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> Result<Json<RemoveCommentResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let (locked, moderator) = lock_for_change(
        &mut transaction,
        &principal,
        project_id,
        work_item_id,
        comment_id,
    )
    .await?;
    let actor = browser_actor(&headers, &principal, project_id)?;
    let already_removed = apply_comment_removal(&mut transaction, &actor, moderator, &locked)
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

/// Whether `actor` may delete other people's comments in a project: an
/// enabled owner or admin, and a person.
pub(crate) async fn is_comment_moderator(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    actor_kind: ActorKind,
    project_id: Uuid,
) -> Result<bool, AppError> {
    if actor_kind != ActorKind::Human {
        return Ok(false);
    }
    // An app acting for them moderates only while its agent limit keeps
    // "moderate channels" too.
    if crate::policy::agent_limit_hides(
        actor_id,
        project_id,
        crate::policy::Permission::ChannelsModerate,
    ) {
        return Ok(false);
    }
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM project_memberships
            WHERE project_id = $1 AND actor_id = $2 AND enabled
              AND role IN ('owner', 'admin')
              AND custom_role_allows(role_id, 'channels.moderate')
        )
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_one(&mut **transaction)
    .await?)
}

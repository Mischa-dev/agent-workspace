import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { searchMentionableActors } from '../conversations/api'
import { draftFromContent } from '../conversations/composerModel'
import { MessageContent } from '../conversations/MessageContent'
import { StructuredComposer } from '../conversations/StructuredComposer'
import type { ConversationActor, MessageContentV1 } from '../conversations/types'
import { Banner, Button, ConfirmDialog, Dialog, MessageSkeleton, describeError, describeLoadError } from '../ui'
import { formatDateTime, formatFullDate } from '../ui/time'
import { getWorkItemCommentRevisions, searchWorkItemReferences } from './api'
import { useDeleteWorkItemComment, useEditWorkItemComment } from './hooks'
import type { WorkItemTimelineEntry } from './types'

export type PostedComment = NonNullable<WorkItemTimelineEntry['comment']>

/** Said wherever someone edits a comment or reads its history: editing never hides old text. */
const COMMENT_HISTORY_HINT = 'Earlier versions stay visible to everyone who can see this task. To take down a secret, delete the comment.'

function mentioned(comment: PostedComment): ConversationActor[] {
  return comment.mentionedActors.map((actor) => ({ ...actor, kind: actor.kind as ConversationActor['kind'], avatarUrl: null }))
}

function editError(reason: unknown) {
  if (!isApiError(reason)) return describeError(reason).message
  if (reason.status === 403) return 'You can only edit your own comments.'
  if (reason.status === 404) return 'That comment wasn’t found, or you can’t see it.'
  if (reason.code === 'version_conflict') return 'This comment changed somewhere else, maybe through a connected app. Copy your text, then reopen the editor.'
  if (reason.status === 409) return 'This comment can’t be edited any more.'
  if (reason.status === 422) return 'That isn’t a comment that can be saved. Check its length and mentions.'
  return describeError(reason).message
}

/** The inline editor that replaces a comment's body, saving against the revision it opened at. */
export function CommentEditor({
  comment,
  onDone,
  projectId,
  workItemId,
}: {
  comment: PostedComment
  onDone: () => void
  projectId: string
  workItemId: string
}) {
  const edit = useEditWorkItemComment()
  const [draft] = useState(() => draftFromContent({ ...comment, mentionedActors: mentioned(comment) }))
  const [revision] = useState(comment.revision)
  return (
    <StructuredComposer
      autoFocus
      className="aw-msg__editor"
      initialDraft={draft}
      keepDraftWhileSaving
      label="Edit comment"
      note={COMMENT_HISTORY_HINT}
      onCancel={onDone}
      onSubmit={async (submission) => {
        try {
          await edit.mutateAsync({ projectId, workItemId, commentId: comment.id, content: submission.content, expectedRevision: revision })
          onDone()
        } catch (reason) {
          throw new Error(editError(reason), { cause: reason })
        }
      }}
      placeholder="Edit comment"
      searchActors={async (query) => (await searchMentionableActors(projectId, query)).actors}
      searchWorkItems={(query) => searchWorkItemReferences(projectId, query)}
      submitIcon="check"
      submitLabel="Save"
    />
  )
}

/** Confirms deleting a comment, as its author or as a project owner or admin. */
export function DeleteCommentDialog({
  comment,
  onClose,
  projectId,
  viewerId,
  workItemId,
}: {
  comment: PostedComment
  onClose: () => void
  projectId: string
  viewerId: string | null
  workItemId: string
}) {
  const remove = useDeleteWorkItemComment()
  const own = comment.author.id === viewerId
  const description = 'Everyone will see “Comment deleted” instead. Its text, earlier versions and attached files go and can’t be brought back.'
  const error = remove.error
    ? isApiError(remove.error) && remove.error.status === 403
      ? 'Only project owners and admins can delete other people’s comments.'
      : `The comment wasn’t deleted. ${describeError(remove.error).message}`
    : null
  return (
    <ConfirmDialog
      busy={remove.isPending}
      confirmLabel="Delete comment"
      error={error}
      onClose={onClose}
      onConfirm={() => remove.mutate({ projectId, workItemId, commentId: comment.id }, { onSuccess: onClose })}
      open
      title={own ? 'Delete this comment?' : `Delete ${comment.author.displayName}’s comment?`}
    >
      {own ? description : `${description} They’ll see that you deleted it.`}
    </ConfirmDialog>
  )
}

function revisionLabel(revision: number) {
  return revision === 1 ? 'Original' : `Edit ${revision - 1}`
}

/** Whether an old version's mentions and references can still be drawn; if not, its plain text is shown. */
function resolvable(content: MessageContentV1, comment: PostedComment) {
  const actors = new Set(comment.mentionedActors.map((actor) => actor.id))
  const items = new Set(comment.referencedWorkItems.map((item) => item.id))
  return content.nodes.every(
    (node) =>
      node.type === 'text' ||
      (node.type === 'mention' && actors.has(node.actorId)) ||
      (node.type === 'work_item_ref' && items.has(node.workItemId)),
  )
}

/** Every version of a comment, oldest first, with the current one marked. */
export function CommentHistoryDialog({
  comment,
  onClose,
  projectId,
  workItemId,
}: {
  comment: PostedComment
  onClose: () => void
  projectId: string
  workItemId: string
}) {
  const history = useQuery({
    queryKey: queryKeys.workItemCommentRevisions(projectId, workItemId, comment.id),
    queryFn: () => getWorkItemCommentRevisions(projectId, workItemId, comment.id),
    retry: false,
  })
  const data = history.data
  return (
    <Dialog description={COMMENT_HISTORY_HINT} footer={<Button onClick={onClose}>Close</Button>} onClose={onClose} open title="Edit history">
      {history.isPending ? <MessageSkeleton label="Loading earlier versions" rows={2} /> : null}
      {history.isError ? (
        <Banner onRetry={() => void history.refetch()} title="The earlier versions didn’t load." tone="danger">
          {describeLoadError(history.error)}
        </Banner>
      ) : null}
      {data?.removed ? <Banner tone="offline">This comment was deleted, and the text of every version with it.</Banner> : null}
      {data && data.revisions.length > 0 ? (
        <ol className="aw-history">
          {data.revisions.map((revision) => (
            <li className="aw-history__item" key={revision.revision}>
              <p className="aw-history__meta">
                <strong>{revisionLabel(revision.revision)}</strong>
                <span aria-hidden="true"> · </span>
                <time dateTime={revision.writtenAt} title={formatFullDate(revision.writtenAt)}>
                  {formatDateTime(revision.writtenAt)}
                </time>
                {!data.removed && revision.revision === data.revision ? <span className="aw-history__current">Current</span> : null}
              </p>
              {revision.redacted || !revision.content ? (
                <p className="aw-history__redacted">Text removed</p>
              ) : (
                <div className="aw-history__body">
                  <MessageContent
                    actors={mentioned(comment)}
                    content={resolvable(revision.content, comment) ? revision.content : null}
                    fallbackPlainText={revision.plainText ?? ''}
                    workItems={comment.referencedWorkItems}
                  />
                </div>
              )}
            </li>
          ))}
        </ol>
      ) : null}
    </Dialog>
  )
}

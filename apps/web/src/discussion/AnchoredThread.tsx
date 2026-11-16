import { useQueryClient } from '@tanstack/react-query'

import { callOp, invalidateOps } from '../api/ops'
import type { CommentThreadView, SubjectKind } from '../api/generated/operations'
import { queryKeys } from '../api/queryKeys'
import { searchMentionableActors } from '../conversations/api'
import type { ComposerSubmission } from '../conversations/composerModel'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { TimelineEntries } from '../conversations/Timeline'
import { useConversationTimeline } from '../conversations/useConversationTimeline'
import { Button, ErrorState, MessageSkeleton } from '../ui'
import '../styles/conversation.css'

export type AnchoredThreadProps = {
  kind: SubjectKind
  thingId: string
  viewerId: string
  /** The thing's space, from `comment_list`. */
  spaceId: string
  /** The thread about this place, or none yet. */
  thread: CommentThreadView | null
  /** Where a new thread is anchored, such as a cell. */
  anchor: unknown
  /** Whether new comments are taken (not when the thing is archived). */
  acceptsComments: boolean
}

/**
 * One anchored discussion on a thing (C1.19): the thread about a place in
 * it, such as a cell, and a composer. The first comment starts the thread
 * there; later ones reply in it.
 */
export default function AnchoredThread({ acceptsComments, anchor, kind, spaceId, thingId, thread, viewerId }: AnchoredThreadProps) {
  const queryClient = useQueryClient()
  const thing = { kind, id: thingId }
  const timeline = useConversationTimeline(spaceId, thread?.id ?? '')

  const refresh = async () => {
    await invalidateOps(queryClient, ['comment_list'])
    if (spaceId && thread) await queryClient.invalidateQueries({ queryKey: queryKeys.timeline(spaceId, thread.id, false) })
  }

  const submit = async (submission: ComposerSubmission) => {
    await callOp(
      'comment_create',
      {
        thing,
        content: submission.content,
        attachmentIds: submission.attachments?.map((attachment) => attachment.id),
        ...(thread ? { threadId: thread.id } : { anchor }),
      },
      { requestId: submission.idempotencyKey },
    )
    await refresh()
  }

  const setStatus = async (status: string) => {
    if (!thread) return
    await callOp('comment_thread_update', { threadId: thread.id, status })
    await refresh()
  }

  const resolved = thread?.status.toLowerCase() === 'resolved'
  return (
    <section aria-label="Comments here" className="aw-thingcomments">
      {thread && timeline.isPending ? <MessageSkeleton label="Loading comments" rows={2} /> : null}
      {thread && timeline.isError ? (
        <ErrorState error={timeline.error} onRetry={() => void timeline.refetch()} title="Comments didn’t load" />
      ) : null}
      {thread && timeline.data ? <TimelineEntries conversationId={thread.id} entries={timeline.data.entries} viewerId={viewerId} /> : null}
      {!thread ? <p className="aw-thingcomments__empty">No comments here yet.</p> : null}
      {acceptsComments ? (
        <StructuredComposer
          label="Comment"
          onSubmit={submit}
          placeholder={thread ? 'Reply' : 'Comment'}
          searchActors={async (query) =>
            spaceId ? (await searchMentionableActors(spaceId, query)).actors.filter((actor) => actor.id !== viewerId) : []
          }
          submitLabel={thread ? 'Reply' : 'Comment'}
        />
      ) : null}
      {thread && acceptsComments ? (
        <Button onClick={() => void setStatus(resolved ? 'Open' : 'Resolved')} size="sm" variant="ghost">
          {resolved ? 'Reopen' : 'Resolve'}
        </Button>
      ) : null}
    </section>
  )
}

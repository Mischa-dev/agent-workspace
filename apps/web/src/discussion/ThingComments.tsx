import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, invalidateOps, opQuery } from '../api/ops'
import type { SubjectKind } from '../api/generated/operations'
import { queryKeys } from '../api/queryKeys'
import { searchMentionableActors } from '../conversations/api'
import type { ComposerSubmission } from '../conversations/composerModel'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { ErrorState, MessageSkeleton } from '../ui'
import { DiscussionEntries } from './DiscussionEntries'
import '../styles/conversation.css'

export type ThingCommentsProps = {
  kind: SubjectKind
  thingId: string
  viewerId: string
}

/**
 * A thing's comments (C1.3): its comment stream, its threads and a composer.
 * Comments are messages, so the stream is the conversation timeline; the
 * first comment opens the conversation.
 */
export default function ThingComments({ kind, thingId, viewerId }: ThingCommentsProps) {
  const queryClient = useQueryClient()
  const thing = { kind, id: thingId }
  const summary = useQuery(opQuery('comment_list', { thing }))
  const spaceId = summary.data?.spaceId ?? ''
  const conversationId = summary.data?.conversationId ?? ''

  const submit = async (submission: ComposerSubmission) => {
    await callOp(
      'comment_create',
      {
        thing,
        content: submission.content,
        attachmentIds: submission.attachments?.map((attachment) => attachment.id),
        replyToMessageId: submission.replyTo?.messageId ?? null,
      },
      { requestId: submission.idempotencyKey },
    )
    await invalidateOps(queryClient, ['comment_list'])
    if (spaceId && conversationId) {
      await queryClient.invalidateQueries({ queryKey: queryKeys.timeline(spaceId, conversationId, false) })
    }
  }

  if (summary.isPending) return <MessageSkeleton label="Loading comments" rows={2} />
  if (summary.isError) {
    return <ErrorState error={summary.error} onRetry={() => void summary.refetch()} title="Comments didn’t load" />
  }
  const threads = summary.data.threads
  return (
    <section aria-label="Comments" className="aw-thingcomments">
      {spaceId && conversationId ? <DiscussionEntries conversationId={conversationId} spaceId={spaceId} viewerId={viewerId} /> : null}
      {threads.length > 0 ? (
        <ul aria-label="Discussions" className="aw-thingcomments__threads">
          {threads.map((thread) => (
            <li key={thread.id}>
              <span>{thread.name}</span> · <span>{thread.status}</span> · <span>{thread.messageCount}</span>
              {thread.unread ? <span className="aw-thingcomments__unread"> · new</span> : null}
            </li>
          ))}
        </ul>
      ) : null}
      {summary.data.acceptsComments ? (
        <StructuredComposer
          label="Comment"
          onSubmit={submit}
          placeholder="Comment"
          searchActors={async (query) =>
            spaceId ? (await searchMentionableActors(spaceId, query)).actors.filter((actor) => actor.id !== viewerId) : []
          }
          submitLabel="Comment"
        />
      ) : (
        <StructuredComposer
          disabledReason="This is archived. You can read its comments, but no one can add one."
          onSubmit={async () => undefined}
          searchActors={async () => []}
        />
      )}
    </section>
  )
}

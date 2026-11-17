import { getConversationTimeline } from '../conversations/api'
import type { ConversationTimelineEntry } from '../conversations/types'
import { OlderMessagesButton, TimelineEntries } from '../conversations/Timeline'
import { useConversationTimeline } from '../conversations/useConversationTimeline'
import { useLiveOlderEntries } from '../conversations/useLiveOlderEntries'
import { Banner, ErrorState, MessageSkeleton } from '../ui'

const NO_ENTRIES: ConversationTimelineEntry[] = []

/**
 * A discussion's comments, oldest first: the newest page, and earlier ones
 * on "Load earlier messages", kept current while open.
 */
export function DiscussionEntries({ conversationId, spaceId, viewerId }: { conversationId: string; spaceId: string; viewerId: string }) {
  const timeline = useConversationTimeline(spaceId, conversationId)
  const page = timeline.data?.conversation.id === conversationId ? timeline.data : null
  const older = useLiveOlderEntries({
    conversationId,
    newestEntries: page?.entries ?? NO_ENTRIES,
    newestCursor: page?.nextCursor ?? null,
    loadPage: async (cursor) => {
      const result = await getConversationTimeline(conversationId, cursor, 50)
      if (result.conversation.id !== conversationId || result.conversation.projectId !== spaceId) {
        throw new Error('Older comments crossed their discussion.')
      }
      return result
    },
  })
  if (timeline.isPending) return <MessageSkeleton label="Loading comments" rows={2} />
  if (timeline.isError) {
    return <ErrorState error={timeline.error} onRetry={() => void timeline.refetch()} title="Comments didn’t load" />
  }
  return (
    <>
      {older.cursor ? <OlderMessagesButton busy={older.loading} onClick={() => void older.loadOlder()} /> : null}
      {older.error ? (
        <Banner onRetry={() => void older.loadOlder()} title="Earlier comments didn’t load." tone="danger">
          {older.error}
        </Banner>
      ) : null}
      <TimelineEntries conversationId={conversationId} entries={older.entries} viewerId={viewerId} />
    </>
  )
}

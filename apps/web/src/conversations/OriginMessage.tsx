import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { getConversationMessage } from './api'
import { MessageReactions } from './Reactions'
import { MessageRow } from './Timeline'
import type { ConversationTimelineEntry } from './types'

/**
 * The message a thread was started from. Kept under its channel's timeline
 * key, so a live edit, removal or reaction in the channel shows here too.
 */
function useOriginMessage(projectId: string, parentId: string | null, originId: string | null) {
  return useQuery({
    queryKey: ['project', projectId, 'conversation', parentId ?? '', 'timeline', 'origin', originId ?? ''],
    queryFn: async (): Promise<{ entries: ConversationTimelineEntry[] }> => {
      const entry = await getConversationMessage(parentId ?? '', originId ?? '')
      return { entries: entry ? [entry] : [] }
    },
    enabled: Boolean(projectId && parentId && originId),
    retry: false,
    staleTime: 30_000,
  })
}

/**
 * At the top of a thread, as Slack shows it: the message it was started
 * from, read-only, with where it was said and a way back to it.
 */
export function OriginMessage({
  channelHref,
  channelName,
  direct = false,
  originId,
  parentId,
  projectId,
  viewerId,
}: {
  /** The parent as a page; the link lands on the message. */
  channelHref: string
  /** "#general", or a group's name. */
  channelName?: string | null
  /** Under a DM or group rather than a channel. */
  direct?: boolean
  originId: string | null
  parentId: string | null
  projectId: string
  viewerId: string
}) {
  const origin = useOriginMessage(projectId, parentId, originId)
  const entry = origin.data?.entries[0]
  if (!originId || !parentId) return null
  if (!entry?.message) {
    // Still loading, or you can't see it: the thread reads fine without it.
    return null
  }
  const message = entry.message
  return (
    <section aria-label="Started from" className="aw-origin">
      <p className="aw-origin__where">
        Started from a message{channelName ? ` in ${channelName}` : ''}
        {message.removed ? null : (
          <>
            {' · '}
            <Link to={`${channelHref}#message-${encodeURIComponent(message.id)}`}>{direct ? 'View in conversation' : 'View in channel'}</Link>
          </>
        )}
      </p>
      <MessageRow
        anchor={false}
        entry={entry}
        reactions={message.reactions?.length ? <MessageReactions reactions={message.reactions} viewerId={viewerId} /> : null}
        viewerId={viewerId}
      />
    </section>
  )
}

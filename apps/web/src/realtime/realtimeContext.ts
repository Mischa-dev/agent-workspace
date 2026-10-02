import { createContext, useContext } from 'react'

import type { MessageUpdates, ReactionUpdates } from '../conversations/liveMessages'

export type ConnectionStatus = 'connecting' | 'live' | 'retrying'

export type RealtimeContextValue = {
  connection: ConnectionStatus
  streams: Record<string, Record<string, string>>
  /** Messages edited or removed while connected, by message id (the latest 200). */
  messageUpdates: MessageUpdates
  /** Messages whose reactions changed while connected, by message id (the latest 200). */
  reactionUpdates: ReactionUpdates
  /**
   * Goes up each time the socket (re)connects. Anything kept outside the query
   * cache, such as older timeline pages, refetches then: events sent while the
   * socket was down never arrive.
   */
  resyncGeneration: number
}

export const RealtimeContext = createContext<RealtimeContextValue | null>(null)

export function useRealtime() {
  const realtime = useContext(RealtimeContext)
  if (!realtime) {
    throw new Error('Realtime state rendered outside an authenticated session.')
  }
  return realtime
}

export function useOptionalRealtime() {
  return useContext(RealtimeContext)
}

export function useConversationRealtime(conversationId: string) {
  const realtime = useRealtime()
  return {
    connection: realtime.connection,
    streaming: realtime.streams[conversationId] ?? {},
  }
}

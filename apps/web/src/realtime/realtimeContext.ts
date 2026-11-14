import { createContext, useContext, useSyncExternalStore } from 'react'

import { EMPTY_LIVE_STORE, type LiveStore } from './liveStore'

export type ConnectionStatus = 'connecting' | 'live' | 'retrying'

/**
 * The stable part of the realtime state: it changes only when the socket
 * connects or drops, so reading it never renders a component per streamed
 * frame. What changes faster lives in `live` and is read with the hooks
 * below (`useStream`, `useMessageUpdates`, `useReactionUpdates`).
 */
export type RealtimeContextValue = {
  connection: ConnectionStatus
  /**
   * Goes up each time the socket (re)connects. Anything kept outside the query
   * cache, such as older timeline pages, refetches then: events sent while the
   * socket was down never arrive.
   */
  resyncGeneration: number
  /** Streaming text, edits and reactions, read through the hooks below. */
  live: LiveStore
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

const NO_STREAM: Record<string, string> = {}

function useLive<T>(select: (store: LiveStore) => T): T {
  const store = useContext(RealtimeContext)?.live ?? EMPTY_LIVE_STORE
  return useSyncExternalStore(store.subscribe, () => select(store))
}

/** Text streaming into a conversation, by message id. */
export function useStream(conversationId: string) {
  return useLive((store) => store.get().streams[conversationId] ?? NO_STREAM)
}

/** Messages edited or removed while connected. */
export function useMessageUpdates() {
  return useLive((store) => store.get().messageUpdates)
}

/** Messages whose reactions changed while connected. */
export function useReactionUpdates() {
  return useLive((store) => store.get().reactionUpdates)
}

export function useConversationRealtime(conversationId: string) {
  const realtime = useRealtime()
  return {
    connection: realtime.connection,
    streaming: useStream(conversationId),
  }
}

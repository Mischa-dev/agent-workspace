import { createContext, useContext, useEffect, useRef, useSyncExternalStore } from 'react'

import type { DocumentRef } from './live/protocol'
import { EMPTY_LIVE_STORE, type LiveStore } from './liveStore'

export type ConnectionStatus = 'connecting' | 'live' | 'retrying'

/** What others do in a watched document: a change to apply, or a cursor. */
export type DocumentFrame =
  | { type: 'document.update'; generation: string; version: number; update: string }
  | { type: 'awareness'; connection: string; accountId: string; state: unknown }

/**
 * What the page sends over the live connection (protocol 2). On a server
 * without it these send nothing that matters: it ignores them.
 */
export type LiveControls = {
  /** Say the signed-in person is typing in a conversation; sent at most every 3 s. */
  typing: (conversationId: string) => void
  /**
   * Watch a document while it is open: its changes and its watchers' cursors
   * come to `listener`. Watches are counted, so two views of one page watch
   * it once. Returns how to stop.
   */
  watch: (document: DocumentRef, listener?: (frame: DocumentFrame) => void) => () => void
  /** Share this person's cursor or selection in a watched document. */
  awareness: (document: DocumentRef, state: unknown) => void
}

export const NO_LIVE_CONTROLS: LiveControls = {
  typing: () => undefined,
  watch: () => () => undefined,
  awareness: () => undefined,
}

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
  /** What the page sends: typing, document watches, cursors. */
  controls: LiveControls
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
const NO_TYPERS: Record<string, number> = {}

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

/** Who is typing in a conversation now, by account id (until when, in ms). */
export function useTyping(conversationId: string) {
  return useLive((store) => store.get().typing[conversationId] ?? NO_TYPERS)
}

/** Whether an account is online; undefined until the server has said. */
export function usePresence(accountId: string): boolean | undefined {
  return useLive((store) => store.get().presence[accountId])
}

/** What the page sends over the live connection; nothing outside a session. */
export function useLiveControls(): LiveControls {
  return useContext(RealtimeContext)?.controls ?? NO_LIVE_CONTROLS
}

/**
 * Watch `document` while the calling component shows it (null watches
 * nothing). `listener` may change between renders without re-watching.
 */
export function useWatchDocument(document: DocumentRef | null, listener?: (frame: DocumentFrame) => void) {
  const controls = useLiveControls()
  const latest = useRef(listener)
  useEffect(() => {
    latest.current = listener
  })
  const kind = document?.kind
  const id = document?.id
  useEffect(() => {
    if (!kind || !id) return
    return controls.watch({ kind, id }, (frame) => latest.current?.(frame))
  }, [controls, kind, id])
}

export function useConversationRealtime(conversationId: string) {
  const realtime = useRealtime()
  return {
    connection: realtime.connection,
    streaming: useStream(conversationId),
  }
}

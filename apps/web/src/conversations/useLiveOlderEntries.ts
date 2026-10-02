import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import { useOptionalRealtime } from '../realtime/realtimeContext'
import { getConversationMessage } from './api'
import {
  applyLiveMessages,
  isNewerMessage,
  mergeTimelineEntries,
  needsRefresh,
  NO_MESSAGE_UPDATES,
} from './liveMessages'
import type { ConversationTimelineEntry } from './types'

export type OlderPage = { entries: ConversationTimelineEntry[]; nextCursor: string | null }

type LoadedPage = { cursor: string; entries: ConversationTimelineEntry[] }
type OlderState = { conversationId: string; pages: LoadedPage[]; nextCursor: string | null }
type Replacements = { conversationId: string; byMessageId: Record<string, ConversationTimelineEntry> }

const NO_OLDER: OlderState = { conversationId: '', pages: [], nextCursor: null }
const NO_REPLACEMENTS: Record<string, ConversationTimelineEntry> = {}
const NO_REPLACEMENT_STATE: Replacements = { conversationId: '', byMessageId: NO_REPLACEMENTS }
const NO_PAGES: LoadedPage[] = []

/**
 * The timeline as shown: the newest page (from the query cache) plus the pages
 * loaded with "Load earlier messages", kept current while it's open.
 *
 * Older pages live in component state, so a project refresh never refetches
 * them. Instead:
 * - a removal seen live blanks the message during render, before any fetch;
 * - an edit or removal seen live refetches that one message;
 * - each time the socket reconnects (events sent while it was down never
 *   arrive), every older page is refetched, or dropped if that fails;
 * - moving to another conversation drops them. Pages keep the same component
 *   across conversations, and a reconnect while elsewhere doesn't refetch
 *   them, so pages kept for a return could show text removed meanwhile.
 */
export function useLiveOlderEntries({
  conversationId,
  loadPage,
  newestCursor,
  newestEntries,
}: {
  conversationId: string
  /** Loads the page before `cursor`, after checking it belongs to this conversation. */
  loadPage: (cursor: string) => Promise<OlderPage>
  /** Where the newest page ends; null when it holds the whole conversation. */
  newestCursor: string | null
  newestEntries: ConversationTimelineEntry[]
}) {
  const realtime = useOptionalRealtime()
  const updates = realtime?.messageUpdates ?? NO_MESSAGE_UPDATES
  const resyncGeneration = realtime?.resyncGeneration ?? 0
  const [older, setOlder] = useState<OlderState>(NO_OLDER)
  const [replacements, setReplacements] = useState<Replacements>(NO_REPLACEMENT_STATE)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [shownConversationId, setShownConversationId] = useState(conversationId)
  const loadPageRef = useRef(loadPage)
  const olderRef = useRef(older)
  const attempted = useRef(new Set<string>())
  const seenGeneration = useRef(resyncGeneration)
  // Goes up with each conversation shown, so a page load answered after
  // leaving is never kept.
  const visit = useRef(0)
  useEffect(() => {
    loadPageRef.current = loadPage
    olderRef.current = older
  })

  // Another conversation: forget everything loaded for the last one, so a
  // return starts from the newest page instead of pages that missed a resync.
  if (shownConversationId !== conversationId) {
    setShownConversationId(conversationId)
    setOlder(NO_OLDER)
    setReplacements(NO_REPLACEMENT_STATE)
    setLoading(false)
    setError(null)
  }
  useEffect(() => {
    visit.current += 1
    attempted.current.clear()
  }, [conversationId])

  const own = older.conversationId === conversationId
  const pages = own ? older.pages : NO_PAGES
  const cursor = own && pages.length > 0 ? older.nextCursor : newestCursor
  const byMessageId = replacements.conversationId === conversationId ? replacements.byMessageId : NO_REPLACEMENTS

  const olderEntries = useMemo(() => pages.flatMap((page) => page.entries), [pages])
  const entries = useMemo(
    () => applyLiveMessages(mergeTimelineEntries(olderEntries, newestEntries), updates, byMessageId, conversationId),
    [byMessageId, conversationId, newestEntries, olderEntries, updates],
  )

  const replaceEntry = useCallback(
    (entry: ConversationTimelineEntry) => {
      const message = entry.message
      if (!message) return
      setReplacements((current) => {
        const kept = current.conversationId === conversationId ? current.byMessageId : {}
        const existing = kept[message.id]?.message
        if (existing && !isNewerMessage(message, existing)) return current
        return { conversationId, byMessageId: { ...kept, [message.id]: entry } }
      })
    },
    [conversationId],
  )

  // Refetch older messages that live updates say changed. The newest page is
  // refetched by the project refresh, so only older ones need this.
  useEffect(() => {
    const newestIds = new Set(newestEntries.map((entry) => entry.message?.id))
    for (const entry of olderEntries) {
      const base = entry.message
      if (!base || newestIds.has(base.id)) continue
      const replacement = byMessageId[base.id]?.message
      const message = replacement && isNewerMessage(replacement, base) ? replacement : base
      const update = updates[base.id]
      if (!update || update.conversationId !== conversationId || !needsRefresh(message, update)) continue
      const key = `${conversationId}:${base.id}:${update.revision}:${update.removed}`
      if (attempted.current.has(key)) continue
      attempted.current.add(key)
      void getConversationMessage(conversationId, base.id)
        .then(replaceEntry)
        // The blanking above already hides a removed message's text; a failed
        // fetch of an edit leaves the old text until the next reconnect.
        .catch(() => undefined)
    }
  }, [byMessageId, conversationId, newestEntries, olderEntries, replaceEntry, updates])

  // On every reconnect, refetch each older page by the cursor that loaded it,
  // or drop them all when that fails, so nothing missed while offline stays.
  useEffect(() => {
    if (seenGeneration.current === resyncGeneration) return
    seenGeneration.current = resyncGeneration
    const snapshot = olderRef.current
    if (snapshot.conversationId !== conversationId || snapshot.pages.length === 0) return
    let cancelled = false
    void Promise.all(
      snapshot.pages.map((page) =>
        loadPageRef.current(page.cursor).then((result) => ({ cursor: page.cursor, ...result })),
      ),
    )
      .then((results) => {
        if (cancelled) return
        const fresh = new Map(results.map((result) => [result.cursor, result]))
        attempted.current.clear()
        setOlder((current) => {
          if (current.conversationId !== conversationId) return current
          const last = current.pages.at(-1)
          return {
            conversationId,
            pages: current.pages.map((page) => {
              const result = fresh.get(page.cursor)
              return result ? { cursor: page.cursor, entries: result.entries } : page
            }),
            nextCursor: last && fresh.has(last.cursor) ? fresh.get(last.cursor)?.nextCursor ?? null : current.nextCursor,
          }
        })
      })
      .catch(() => {
        if (cancelled) return
        setOlder((current) => (current.conversationId === conversationId ? NO_OLDER : current))
      })
    return () => {
      cancelled = true
    }
  }, [conversationId, resyncGeneration])

  const loadOlder = useCallback(async () => {
    if (!cursor || loading) return
    const started = visit.current
    setLoading(true)
    setError(null)
    try {
      const result = await loadPageRef.current(cursor)
      if (visit.current !== started) return
      setOlder((current) => ({
        conversationId,
        pages: [...(current.conversationId === conversationId ? current.pages : []), { cursor, entries: result.entries }],
        nextCursor: result.nextCursor,
      }))
    } catch (reason) {
      if (visit.current !== started) return
      setError(reason instanceof Error ? reason.message : 'Earlier messages didn’t load.')
    } finally {
      if (visit.current === started) setLoading(false)
    }
  }, [conversationId, cursor, loading])

  return {
    /** Every loaded entry, oldest first, with live edits and removals applied. */
    entries,
    /** The cursor "Load earlier messages" uses next; null when there's nothing older. */
    cursor,
    loading,
    error,
    loadOlder,
    /** Show a changed message right away, for example after your own edit. */
    replaceEntry,
    olderCount: olderEntries.length,
  }
}

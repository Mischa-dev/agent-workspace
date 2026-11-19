import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { origins } from '../api/client'
import { currentQueryScopeGeneration } from '../api/queryScope'
import { coalesceDeltaBatch } from '../app/streaming'
import {
  dropRemovedConversation,
  dropRemovedMessage,
  refreshMemoryCaches,
  rememberMessageUpdate,
  rememberReactionUpdate,
  type MessageUpdates,
  type ReactionUpdates,
} from '../conversations/liveMessages'
import { cacheContext, RECONNECT, SELF_HANDLED, type Touch } from './handlers'
import { accessChanged } from './live/access'
import {
  after,
  awarenessFrame,
  documentKey,
  hello,
  parseServerFrame,
  typingFrame,
  unwatchFrame,
  watchFrame,
  type DocumentRef,
  type ServerFrame,
} from './live/protocol'
import { createLiveStore, NO_PRESENCE, NO_STREAMS, NO_TYPING, type Streams, type Typing } from './liveStore'
import { modulesChanged } from './moduleSwitches'
import { NO_LIVE_CONTROLS, type ConnectionStatus, type DocumentFrame, type LiveControls, type RealtimeContextValue } from './realtimeContext'
import {
  durableMessageId,
  messageUpdate,
  parseLegacyRealtimeEvent,
  reactionChange,
  parseRealtimeEnvelope,
  removedConversationId,
  streamDelta,
  terminalAgentRunId,
  type RealtimeEnvelope,
} from './realtimeModel'
import { workItemChange } from './workItemEvent'

const MAX_RECENT_EVENT_IDS = 512
/** A burst of events refetches each touched query once. */
const INVALIDATION_DELAY_MS = 40
const MAX_RECONNECT_DELAY_MS = 10_000
// Phones suspend a page in the background, and its socket can die without a
// close event. After this long away, reconnect on return rather than trust it.
const RESUME_RECONNECT_AFTER_MS = 30_000
/**
 * After a resuming `hello`, how long to wait for `ready` before catching up
 * the broad way. The server answers at once, after replaying at most 5,000
 * events.
 */
const RESUME_WAIT_MS = 10_000
/** The server takes one typing frame a conversation every 3 s. */
const TYPING_EVERY_MS = 3_000

function websocketUrl() {
  // The live origin from client-config (H1.41), as ws or wss; protocol 2
  // (W1.6). A server without it ignores `v` and sends protocol 1.
  return `${(origins.live ?? window.location.origin).replace(/^http/, 'ws')}/api/v1/realtime?v=2`
}

type Watch = { document: DocumentRef; count: number; listeners: Set<(frame: DocumentFrame) => void> }

/**
 * Owns the authenticated socket lifecycle and bounded ephemeral stream state.
 * Durable application state remains in TanStack Query and converges by HTTP.
 */
export function useRealtimeConnection(): RealtimeContextValue {
  const queryClient = useQueryClient()
  const [connection, setConnection] =
    useState<ConnectionStatus>('connecting')
  // Streams, edits and reactions change many times a second: they go to
  // an external store so only the components reading them render.
  const [live] = useState(() => createLiveStore())
  const [resyncGeneration, setResyncGeneration] = useState(0)
  const reconnectAttempt = useRef(0)
  // What pages send goes to whichever socket is open; the controls stay the
  // same object for the provider's life.
  const send = useRef<{ controls: LiveControls }>({ controls: NO_LIVE_CONTROLS })
  const [controls] = useState<LiveControls>(() => ({
    typing: (conversationId) => send.current.controls.typing(conversationId),
    watch: (document, listener) => send.current.controls.watch(document, listener),
    awareness: (document, state) => send.current.controls.awareness(document, state),
  }))

  useEffect(() => {
    const setStreams = (change: Streams | ((current: Streams) => Streams)) =>
      live.update((current) => {
        const streams = typeof change === 'function' ? change(current.streams) : change
        return streams === current.streams ? current : { ...current, streams }
      })
    const setMessageUpdates = (change: (current: MessageUpdates) => MessageUpdates) =>
      live.update((current) => {
        const messageUpdates = change(current.messageUpdates)
        return messageUpdates === current.messageUpdates ? current : { ...current, messageUpdates }
      })
    const setReactionUpdates = (change: (current: ReactionUpdates) => ReactionUpdates) =>
      live.update((current) => {
        const reactionUpdates = change(current.reactionUpdates)
        return reactionUpdates === current.reactionUpdates
          ? current
          : { ...current, reactionUpdates }
      })
    const setTyping = (change: (current: Typing) => Typing) =>
      live.update((current) => {
        const typing = change(current.typing)
        return typing === current.typing ? current : { ...current, typing }
      })
    let disposed = false
    let socket: WebSocket | undefined
    let reconnectTimer: number | undefined
    let streamFrame: number | undefined
    // Protocol 2: the last log position heard, kept across reconnects so the
    // next `hello` resumes from it; whether this server speaks protocol 2;
    // the wait for `ready` after a resuming hello; and when the server asked
    // to be called back.
    let position: string | null = null
    let speaksProtocol2 = false
    let resumeTimer: number | undefined
    let plannedDelay: number | undefined
    let typingTimer: number | undefined
    const watches = new Map<string, Watch>()
    const typingSentAt = new Map<string, number>()
    const recentIds = new Set<string>()
    const recentOrder: string[] = []
    const pendingDeltas: Record<string, Record<string, string>> = {}
    const streamRunIds: Record<string, Record<string, string>> = {}
    const invalidationTimers = new Map<string, number>()
    let workItemQueue: Promise<void> = Promise.resolve()

    const remember = (eventId: string) => {
      if (recentIds.has(eventId)) return false
      recentIds.add(eventId)
      recentOrder.push(eventId)
      if (recentOrder.length > MAX_RECENT_EVENT_IDS) {
        const oldest = recentOrder.shift()
        if (oldest) recentIds.delete(oldest)
      }
      return true
    }

    const context = cacheContext(queryClient)
    // What each event makes stale loads with the connection, not the first
    // paint; an event that beats it waits for it.
    const touchTable = import('./handlers/table')
    // A load that fails (or is cut off as the page or a test goes away) is
    // handled where each event awaits it; this keeps it from going unhandled.
    touchTable.catch(() => undefined)

    // Each touch refetches once per burst (handlers/index.ts).
    const invalidate = (touches: readonly Touch[]) => {
      for (const touch of touches) {
        if (invalidationTimers.has(touch.id)) continue
        const timer = window.setTimeout(() => {
          invalidationTimers.delete(touch.id)
          void queryClient.invalidateQueries(touch.filters)
        }, INVALIDATION_DELAY_MS)
        invalidationTimers.set(touch.id, timer)
      }
    }

    const refetchAuthenticatedState = () => {
      for (const touch of RECONNECT) void queryClient.invalidateQueries(touch.filters)
    }

    /** After a gap nothing replayed: everything shown refetches, the one broad refresh. */
    const catchUp = () => {
      refetchAuthenticatedState()
      // Removals sent while the socket was down never arrive: refresh the
      // caches that live outside a project, and older timeline pages.
      refreshMemoryCaches(queryClient)
      setResyncGeneration((generation) => generation + 1)
    }

    const sendNow = (frame: string) => {
      if (socket?.readyState === WebSocket.OPEN) socket.send(frame)
    }

    // Typing shows until the server's `until`; the earliest end prunes.
    const pruneTyping = () => {
      typingTimer = undefined
      const now = Date.now()
      let next = Number.POSITIVE_INFINITY
      setTyping((current) => {
        let changed = false
        const kept: Typing = {}
        for (const [conversationId, typers] of Object.entries(current)) {
          const still = Object.fromEntries(Object.entries(typers).filter(([, until]) => until > now))
          for (const until of Object.values(still)) next = Math.min(next, until)
          if (Object.keys(still).length !== Object.keys(typers).length) changed = true
          if (Object.keys(still).length) kept[conversationId] = Object.keys(still).length === Object.keys(typers).length ? typers : still
        }
        return changed ? kept : current
      })
      if (Number.isFinite(next)) typingTimer = window.setTimeout(pruneTyping, Math.max(next - now, 50))
    }
    const typingStopped = (conversationId: string, accountId: string) =>
      setTyping((current) => {
        if (!current[conversationId]?.[accountId]) return current
        const typers = { ...current[conversationId] }
        delete typers[accountId]
        const next = { ...current }
        if (Object.keys(typers).length) next[conversationId] = typers
        else delete next[conversationId]
        return next
      })

    const sender = send.current
    sender.controls = {
      typing: (conversationId) => {
        const now = Date.now()
        if (now - (typingSentAt.get(conversationId) ?? 0) < TYPING_EVERY_MS) return
        typingSentAt.set(conversationId, now)
        sendNow(typingFrame(conversationId))
      },
      watch: (document, listener) => {
        const key = documentKey(document)
        let watch = watches.get(key)
        if (!watch) {
          watch = { document, count: 0, listeners: new Set() }
          watches.set(key, watch)
          sendNow(watchFrame(document))
        }
        watch.count += 1
        const own = listener ? (frame: DocumentFrame) => listener(frame) : undefined
        if (own) watch.listeners.add(own)
        let stopped = false
        return () => {
          if (stopped) return
          stopped = true
          const current = watches.get(key)
          if (!current) return
          if (own) current.listeners.delete(own)
          current.count -= 1
          if (current.count > 0) return
          watches.delete(key)
          sendNow(unwatchFrame(current.document))
        }
      },
      awareness: (document, state) => {
        if (watches.has(documentKey(document))) sendNow(awarenessFrame(document, state))
      },
    }

    const toDocument = (document: DocumentRef, frame: DocumentFrame) => {
      for (const listener of [...(watches.get(documentKey(document))?.listeners ?? [])]) listener(frame)
    }

    const queueDelta = (
      conversationId: string,
      messageId: string,
      runId: string | null,
      delta: string,
    ) => {
      if (runId) {
        const runIds = (streamRunIds[conversationId] ??= {})
        const existingRunId = runIds[messageId]
        if (existingRunId && existingRunId !== runId) return
        runIds[messageId] = runId
      }
      const conversation = (pendingDeltas[conversationId] ??= {})
      conversation[messageId] = `${conversation[messageId] ?? ''}${delta}`
      if (streamFrame !== undefined) return
      streamFrame = window.requestAnimationFrame(() => {
        const batch = { ...pendingDeltas }
        for (const key of Object.keys(pendingDeltas)) delete pendingDeltas[key]
        streamFrame = undefined
        setStreams((current) => {
          const next = { ...current }
          for (const [id, messages] of Object.entries(batch)) {
            next[id] = coalesceDeltaBatch(next[id] ?? {}, messages)
          }
          return next
        })
      })
    }

    const clearMessageStream = (
      conversationId: string,
      messageId: string,
    ) => {
      const pending = pendingDeltas[conversationId]
      if (pending) {
        delete pending[messageId]
        if (Object.keys(pending).length === 0) delete pendingDeltas[conversationId]
      }
      const runIds = streamRunIds[conversationId]
      if (runIds) {
        delete runIds[messageId]
        if (Object.keys(runIds).length === 0) delete streamRunIds[conversationId]
      }
      setStreams((current) => {
        const conversation = current[conversationId]
        if (!conversation?.[messageId]) return current
        const next = { ...current }
        const remaining = { ...conversation }
        delete remaining[messageId]
        if (Object.keys(remaining).length === 0) delete next[conversationId]
        else next[conversationId] = remaining
        return next
      })
    }

    const clearRunStreams = (conversationId: string, runId: string) => {
      const runIds = streamRunIds[conversationId]
      if (!runIds) return
      const messageIds = Object.entries(runIds)
        .filter(([, candidateRunId]) => candidateRunId === runId)
        .map(([messageId]) => messageId)
      if (messageIds.length === 0) return
      const pending = pendingDeltas[conversationId]
      for (const messageId of messageIds) {
        if (pending) delete pending[messageId]
        delete runIds[messageId]
      }
      if (pending && Object.keys(pending).length === 0) {
        delete pendingDeltas[conversationId]
      }
      if (Object.keys(runIds).length === 0) delete streamRunIds[conversationId]
      setStreams((current) => {
        const conversation = current[conversationId]
        if (!conversation) return current
        const remaining = { ...conversation }
        for (const messageId of messageIds) delete remaining[messageId]
        const next = { ...current }
        if (Object.keys(remaining).length === 0) delete next[conversationId]
        else next[conversationId] = remaining
        return next
      })
    }

    const resetStreams = () => {
      if (streamFrame !== undefined) {
        window.cancelAnimationFrame(streamFrame)
        streamFrame = undefined
      }
      for (const key of Object.keys(pendingDeltas)) delete pendingDeltas[key]
      for (const key of Object.keys(streamRunIds)) delete streamRunIds[key]
      setStreams(NO_STREAMS)
    }

    const connect = () => {
      if (disposed) return
      setConnection(reconnectAttempt.current === 0 ? 'connecting' : 'retrying')
      const current = new WebSocket(websocketUrl())
      socket = current
      current.addEventListener('open', () => {
        if (socket !== current) return
        reconnectAttempt.current = 0
        setConnection('live')
        resetStreams()
        // `hello` first (a server on protocol 1 ignores it), then the
        // documents still open. A server known to speak protocol 2 is asked
        // to resume from the last position heard, and its `ready` says
        // whether anything needs refetching; otherwise the broad refresh.
        const resume = resumable()
        current.send(hello(resume ? position : null))
        for (const watch of watches.values()) current.send(watchFrame(watch.document))
        if (resume) {
          stopWaitingForResume()
          resumeTimer = window.setTimeout(() => {
            resumeTimer = undefined
            catchUp()
          }, RESUME_WAIT_MS)
        } else catchUp()
      })
      current.addEventListener('message', (event) => {
        if (socket !== current) return
        if (typeof event.data !== 'string' || event.data.length > 70_000) return
        let wire: unknown
        try {
          wire = JSON.parse(event.data) as unknown
        } catch {
          return
        }

        const frame = parseServerFrame(wire)
        if (frame) {
          protocol2(current, frame)
          return
        }
        const envelope = parseRealtimeEnvelope(wire)
        if (envelope) {
          durable(envelope)
          return
        }

        const legacy = parseLegacyRealtimeEvent(wire)
        if (!legacy || !remember(legacy.eventId)) return
        const delta = streamDelta(legacy)
        if (legacy.conversationId && delta) {
          queueDelta(
            legacy.conversationId,
            delta.messageId,
            delta.runId,
            delta.delta,
          )
        }
        // Only stream text is sent outside the durable envelope; anything
        // else there names nothing to refetch.
      })
      current.addEventListener('close', () => {
        // A socket replaced by reconnectNow closes quietly.
        if (disposed || socket !== current) return
        // A server that asked to be called back (draining, or a resync) isn't a failure.
        const planned = plannedDelay
        plannedDelay = undefined
        if (planned === undefined) reconnectAttempt.current += 1
        setConnection('retrying')
        resetStreams()
        setTyping(() => NO_TYPING)
        live.update((state) => (state.presence === NO_PRESENCE ? state : { ...state, presence: NO_PRESENCE }))
        stopWaitingForResume()
        // With a position to resume from, the next `ready` says whether
        // anything needs refetching; otherwise everything shown does.
        if (!resumable()) refetchAuthenticatedState()
        const delay = planned ?? Math.min(
          1_000 * 2 ** reconnectAttempt.current,
          MAX_RECONNECT_DELAY_MS,
        )
        reconnectTimer = window.setTimeout(connect, delay)
      })
    }

    const resumable = () => speaksProtocol2 && position !== null
    const stopWaitingForResume = () => {
      if (resumeTimer !== undefined) window.clearTimeout(resumeTimer)
      resumeTimer = undefined
    }

    /** A protocol 2 frame: the server speaks it, so later connections resume. */
    const protocol2 = (current: WebSocket, frame: ServerFrame) => {
      speaksProtocol2 = true
      switch (frame.type) {
        case 'ready':
          if (after(frame.position, position)) position = frame.position
          if (resumeTimer !== undefined) {
            stopWaitingForResume()
            // Resumed: what was missed came before `ready`, through the
            // handlers. Not resumed (too far back): the broad refresh.
            if (!frame.resumed) catchUp()
          }
          return
        case 'event':
          if (after(frame.position, position)) position = frame.position
          {
            const envelope = parseRealtimeEnvelope(frame.event)
            if (envelope) durable(envelope)
          }
          return
        case 'access':
          accessChanged(queryClient, frame)
          return
        case 'resync':
          // The server dropped this connection's events after `position`
          // and closes it: come straight back and resume from there (a
          // resume too far back refetches).
          if (frame.position) position = frame.position
          plannedDelay = 0
          return
        case 'reconnect':
          // The server is stopping: come back when it says, spread out.
          plannedDelay = frame.afterMs + Math.round(Math.random() * frame.afterMs * 0.5)
          if (current.readyState === WebSocket.OPEN) current.close()
          return
        case 'stream.delta':
          queueDelta(frame.conversationId, frame.messageId, frame.runId, frame.delta)
          return
        case 'typing':
          if (frame.until <= Date.now()) return
          setTyping((typing) => ({ ...typing, [frame.conversationId]: { ...typing[frame.conversationId], [frame.accountId]: frame.until } }))
          if (typingTimer === undefined) typingTimer = window.setTimeout(pruneTyping, Math.max(frame.until - Date.now(), 50))
          return
        case 'presence':
          live.update((state) =>
            state.presence[frame.accountId] === frame.online
              ? state
              : { ...state, presence: { ...state.presence, [frame.accountId]: frame.online } },
          )
          return
        case 'awareness':
          toDocument(frame.document, { type: 'awareness', connection: frame.connection, accountId: frame.accountId, state: frame.state })
          return
        case 'document.update':
          toDocument(frame.document, { type: 'document.update', generation: frame.generation, version: frame.version, update: frame.update })
          return
        default:
          return
      }
    }

    /**
     * One dispatch order for durable events, from either protocol: (1) drop
     * repeats, (2) change caches synchronously, so removed text is gone
     * before any fetch, (3) invalidate so everything converges over HTTP.
     */
    const durable = (envelope: RealtimeEnvelope) => {
      if (!remember(envelope.eventId)) return
      const reactions = reactionChange(envelope)
      if (reactions) {
        setReactionUpdates((current) => rememberReactionUpdate(current, reactions))
      }
      const workItem = workItemChange(envelope)
      if (workItem) {
        // One at a time, in event order: a card's neighbours may be moving too.
        // The patch code loads with the first such event; a sign-out while
        // it loads drops the patch rather than touching the next session.
        const generation = currentQueryScopeGeneration()
        workItemQueue = workItemQueue
          .then(() => import('./workItemPatches'))
          .then(({ applyWorkItemChange }) =>
            generation === currentQueryScopeGeneration()
              ? applyWorkItemChange(queryClient, envelope.projectId, workItem)
              : undefined,
          )
          .catch(() => undefined)
      }
      if (modulesChanged(queryClient, envelope)) return
      if (SELF_HANDLED.has(envelope.type)) {
        // An open board or page fetches what changed itself, and lists
        // (and a collection's records) refetch; that code loads with the
        // first such event.
        void import('./liveDocuments')
          .then(({ documentChanged }) => documentChanged(queryClient, envelope))
          .catch(() => undefined)
        return
      }
      const removedFrom = removedConversationId(envelope)
      if (removedFrom) dropRemovedConversation(queryClient, envelope.projectId, removedFrom)
      const update = messageUpdate(envelope)
      if (update) {
        setMessageUpdates((current) => rememberMessageUpdate(current, update))
        if (update.removed) dropRemovedMessage(queryClient, update)
      }
      const messageId = durableMessageId(envelope)
      const terminalRunId = terminalAgentRunId(envelope)
      if (envelope.conversationId && messageId) {
        clearMessageStream(envelope.conversationId, messageId)
      } else if (envelope.conversationId && terminalRunId) {
        clearRunStreams(envelope.conversationId, terminalRunId)
      }
      void touchTable
        .then(({ touchesFor, announceRunFinished }) => {
          invalidate(touchesFor(envelope, context))
          announceRunFinished(envelope)
        })
        .catch(() => undefined)
      if (update?.removed) refreshMemoryCaches(queryClient)
      // Their message is in: they've stopped typing.
      const author = envelope.type === 'conversation.message.created' ? envelope.payload.authorActorId : undefined
      if (envelope.conversationId && typeof author === 'string') typingStopped(envelope.conversationId, author)
    }

    const reconnectNow = () => {
      if (disposed) return
      if (reconnectTimer !== undefined) {
        window.clearTimeout(reconnectTimer)
        reconnectTimer = undefined
      }
      const previous = socket
      socket = undefined
      previous?.close()
      reconnectAttempt.current = 0
      connect()
    }

    let hiddenAt: number | undefined
    const onVisibilityChange = () => {
      if (document.visibilityState === 'hidden') {
        hiddenAt = Date.now()
        return
      }
      const away = hiddenAt === undefined ? 0 : Date.now() - hiddenAt
      hiddenAt = undefined
      if (socket?.readyState !== WebSocket.OPEN || away > RESUME_RECONNECT_AFTER_MS) reconnectNow()
    }
    const onOnline = () => {
      if (socket?.readyState !== WebSocket.OPEN) reconnectNow()
    }

    connect()
    document.addEventListener('visibilitychange', onVisibilityChange)
    window.addEventListener('online', onOnline)
    return () => {
      disposed = true
      document.removeEventListener('visibilitychange', onVisibilityChange)
      window.removeEventListener('online', onOnline)
      if (reconnectTimer !== undefined) window.clearTimeout(reconnectTimer)
      if (streamFrame !== undefined) window.cancelAnimationFrame(streamFrame)
      if (typingTimer !== undefined) window.clearTimeout(typingTimer)
      stopWaitingForResume()
      sender.controls = NO_LIVE_CONTROLS
      for (const timer of invalidationTimers.values()) window.clearTimeout(timer)
      socket?.close()
    }
  }, [queryClient, live])

  return useMemo(
    () => ({ connection, resyncGeneration, live, controls }),
    [connection, resyncGeneration, live, controls],
  )
}

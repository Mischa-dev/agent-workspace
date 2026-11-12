import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { currentQueryScopeGeneration } from '../api/queryScope'
import { coalesceDeltaBatch } from '../app/streaming'
import {
  dropRemovedConversation,
  dropRemovedMessage,
  invalidateConversation,
  NO_MESSAGE_UPDATES,
  NO_REACTION_UPDATES,
  refreshMemoryCaches,
  rememberMessageUpdate,
  rememberReactionUpdate,
  type MessageUpdates,
  type ReactionUpdates,
} from '../conversations/liveMessages'
import type { ConnectionStatus, RealtimeContextValue } from './realtimeContext'
import {
  durableMessageId,
  messageUpdate,
  parseLegacyRealtimeEvent,
  reactionChange,
  parseRealtimeEnvelope,
  removedConversationId,
  streamDelta,
  terminalAgentRunId,
} from './realtimeModel'
import { workItemChange } from './workItemEvent'

const MAX_RECENT_EVENT_IDS = 512
const INVALIDATION_DELAY_MS = 40
const MAX_RECONNECT_DELAY_MS = 10_000
// Phones suspend a page in the background, and its socket can die without a
// close event. After this long away, reconnect on return rather than trust it.
const RESUME_RECONNECT_AFTER_MS = 30_000

function websocketUrl() {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${protocol}//${window.location.host}/api/v1/realtime`
}

/**
 * Owns the authenticated socket lifecycle and bounded ephemeral stream state.
 * Durable application state remains in TanStack Query and converges by HTTP.
 */
export function useRealtimeConnection(): RealtimeContextValue {
  const queryClient = useQueryClient()
  const [connection, setConnection] =
    useState<ConnectionStatus>('connecting')
  const [streams, setStreams] = useState<
    Record<string, Record<string, string>>
  >({})
  const [messageUpdates, setMessageUpdates] =
    useState<MessageUpdates>(NO_MESSAGE_UPDATES)
  const [reactionUpdates, setReactionUpdates] =
    useState<ReactionUpdates>(NO_REACTION_UPDATES)
  const [resyncGeneration, setResyncGeneration] = useState(0)
  const reconnectAttempt = useRef(0)

  useEffect(() => {
    let disposed = false
    let socket: WebSocket | undefined
    let reconnectTimer: number | undefined
    let streamFrame: number | undefined
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

    const invalidateProject = (projectId: string) => {
      if (invalidationTimers.has(projectId)) return
      const timer = window.setTimeout(() => {
        invalidationTimers.delete(projectId)
        void queryClient.invalidateQueries({ queryKey: queryKeys.projects })
        // DMs and groups live in a Project the client does not list, so
        // refresh the Messages overview on any change.
        void queryClient.invalidateQueries({ queryKey: ['direct'] })
        // The inbox reads names and previews live, so any change can show there.
        void queryClient.invalidateQueries({ queryKey: ['inbox'] })
        void queryClient.invalidateQueries({
          predicate: (query) =>
            query.queryKey[0] === 'project' &&
            query.queryKey[1] === projectId,
        })
      }, INVALIDATION_DELAY_MS)
      invalidationTimers.set(projectId, timer)
    }

    // Reactions change one conversation's timelines and nothing else, so
    // they refetch only those (including a DM thread shown in a side panel).
    const invalidateReactions = (projectId: string, conversationId: string) => {
      const key = `${projectId}:${conversationId}`
      if (invalidationTimers.has(key)) return
      const timer = window.setTimeout(() => {
        invalidationTimers.delete(key)
        invalidateConversation(queryClient, projectId, conversationId)
      }, INVALIDATION_DELAY_MS)
      invalidationTimers.set(key, timer)
    }

    // A changed task is patched into lists, boards and its detail in place.
    // Everything else in the project that might show it converges by HTTP,
    // leaving the patched work-item views alone.
    const invalidateAroundWorkItem = (projectId: string, workItemId: string) => {
      const key = `${projectId}:work-item:${workItemId}`
      if (invalidationTimers.has(key)) return
      const timer = window.setTimeout(() => {
        invalidationTimers.delete(key)
        void queryClient.invalidateQueries({
          queryKey: queryKeys.workItemTimeline(projectId, workItemId),
        })
        void queryClient.invalidateQueries({
          predicate: (query) =>
            query.queryKey[0] === 'project' &&
            query.queryKey[1] === projectId &&
            query.queryKey[2] !== 'work-items',
        })
        // My work lists tasks across projects, so it isn't under the project key.
        void queryClient.invalidateQueries({ queryKey: ['my-work'] })
        // A task embedded in a page as a card.
        void queryClient.invalidateQueries({ queryKey: ['things', 'task', workItemId] })
      }, INVALIDATION_DELAY_MS)
      invalidationTimers.set(key, timer)
    }

    const refetchAuthenticatedState = () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects })
      void queryClient.invalidateQueries({ queryKey: ['direct'] })
      void queryClient.invalidateQueries({
        predicate: (query) => query.queryKey[0] === 'project',
      })
      void queryClient.invalidateQueries({ queryKey: ['inbox'] })
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
      setStreams({})
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
        refetchAuthenticatedState()
        // Removals sent while the socket was down never arrive: refresh the
        // caches that live outside a project, and older timeline pages.
        refreshMemoryCaches(queryClient)
        setResyncGeneration((generation) => generation + 1)
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

        const envelope = parseRealtimeEnvelope(wire)
        if (envelope) {
          // One dispatch order for durable events: (1) drop repeats, (2) change
          // caches synchronously, so removed text is gone before any fetch,
          // (3) invalidate so everything converges over HTTP.
          if (!remember(envelope.eventId)) return
          const reactions = reactionChange(envelope)
          if (reactions) {
            setReactionUpdates((current) => rememberReactionUpdate(current, reactions))
            invalidateReactions(envelope.projectId, reactions.conversationId)
            return
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
            invalidateAroundWorkItem(envelope.projectId, workItem.workItemId)
            return
          }
          if (
            envelope.type === 'whiteboard.updated' ||
            envelope.type === 'page.updated' ||
            envelope.type === 'page.discussion.updated' ||
            envelope.type === 'collection.updated'
          ) {
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
          invalidateProject(envelope.projectId)
          if (update?.removed) refreshMemoryCaches(queryClient)
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
          return
        }
        invalidateProject(legacy.projectId)
      })
      current.addEventListener('close', () => {
        // A socket replaced by reconnectNow closes quietly.
        if (disposed || socket !== current) return
        reconnectAttempt.current += 1
        setConnection('retrying')
        resetStreams()
        refetchAuthenticatedState()
        const delay = Math.min(
          1_000 * 2 ** reconnectAttempt.current,
          MAX_RECONNECT_DELAY_MS,
        )
        reconnectTimer = window.setTimeout(connect, delay)
      })
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
      for (const timer of invalidationTimers.values()) window.clearTimeout(timer)
      socket?.close()
    }
  }, [queryClient])

  return useMemo(
    () => ({ connection, streams, messageUpdates, reactionUpdates, resyncGeneration }),
    [connection, streams, messageUpdates, reactionUpdates, resyncGeneration],
  )
}

import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { coalesceDeltaBatch } from '../app/streaming'
import type { ConnectionStatus, RealtimeContextValue } from './realtimeContext'
import {
  durableMessageId,
  parseLegacyRealtimeEvent,
  parseRealtimeEnvelope,
  streamDelta,
  terminalAgentRunId,
} from './realtimeModel'

const MAX_RECENT_EVENT_IDS = 512
const INVALIDATION_DELAY_MS = 40
const MAX_RECONNECT_DELAY_MS = 10_000

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
        void queryClient.invalidateQueries({
          predicate: (query) =>
            query.queryKey[0] === 'project' &&
            query.queryKey[1] === projectId,
        })
      }, INVALIDATION_DELAY_MS)
      invalidationTimers.set(projectId, timer)
    }

    const refetchAuthenticatedState = () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects })
      void queryClient.invalidateQueries({ queryKey: ['direct'] })
      void queryClient.invalidateQueries({
        predicate: (query) => query.queryKey[0] === 'project',
      })
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
      socket = new WebSocket(websocketUrl())
      socket.addEventListener('open', () => {
        reconnectAttempt.current = 0
        setConnection('live')
        resetStreams()
        refetchAuthenticatedState()
      })
      socket.addEventListener('message', (event) => {
        if (typeof event.data !== 'string' || event.data.length > 70_000) return
        let wire: unknown
        try {
          wire = JSON.parse(event.data) as unknown
        } catch {
          return
        }

        const envelope = parseRealtimeEnvelope(wire)
        if (envelope) {
          if (!remember(envelope.eventId)) return
          const messageId = durableMessageId(envelope)
          const terminalRunId = terminalAgentRunId(envelope)
          if (envelope.conversationId && messageId) {
            clearMessageStream(envelope.conversationId, messageId)
          } else if (envelope.conversationId && terminalRunId) {
            clearRunStreams(envelope.conversationId, terminalRunId)
          }
          invalidateProject(envelope.projectId)
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
      socket.addEventListener('close', () => {
        if (disposed) return
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

    connect()
    return () => {
      disposed = true
      if (reconnectTimer !== undefined) window.clearTimeout(reconnectTimer)
      if (streamFrame !== undefined) window.cancelAnimationFrame(streamFrame)
      for (const timer of invalidationTimers.values()) window.clearTimeout(timer)
      socket?.close()
    }
  }, [queryClient])

  return useMemo(() => ({ connection, streams }), [connection, streams])
}

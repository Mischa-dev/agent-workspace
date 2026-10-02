import { isSafeMessageText } from '../conversations/messageContentModel'

export const REALTIME_ENVELOPE_VERSION = 1

const uuidPattern =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/

export type RealtimeEnvelope = {
  eventId: string
  eventCursor: number
  version: number
  projectId: string
  conversationId: string | null
  type: string
  occurredAt: string
  payload: Record<string, unknown>
}

export type LegacyRealtimeEvent = {
  eventId: string
  projectId: string
  conversationId: string | null
  type: string
  payload: Record<string, unknown>
}

type WireRecord = Record<string, unknown>

function asRecord(value: unknown): WireRecord {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as WireRecord)
    : {}
}

function field(record: WireRecord, camel: string, snake: string) {
  return record[camel] ?? record[snake]
}

function uuid(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const normalized = value.toLowerCase()
  return uuidPattern.test(normalized) ? normalized : null
}

function boundedType(value: unknown): string | null {
  return typeof value === 'string' &&
    value.length > 0 &&
    value.length <= 120 &&
    /^[a-z][a-z0-9_.-]*$/.test(value)
    ? value
    : null
}

function boundedPayload(value: unknown): Record<string, unknown> | null {
  const payload = asRecord(value)
  if (Object.keys(payload).length === 0 && value !== payload) return null
  try {
    return JSON.stringify(payload).length <= 65_536 ? payload : null
  } catch {
    return null
  }
}

export function parseRealtimeEnvelope(value: unknown): RealtimeEnvelope | null {
  const record = asRecord(value)
  const eventId = uuid(field(record, 'eventId', 'event_id'))
  const eventCursor = field(record, 'eventCursor', 'event_cursor')
  const version = record.version
  const projectId = uuid(field(record, 'projectId', 'project_id'))
  const rawConversationId = field(
    record,
    'conversationId',
    'conversation_id',
  )
  const payload = boundedPayload(record.payload)
  const payloadConversationId = payload
    ? field(payload, 'conversationId', 'conversation_id')
    : undefined
  const selectedConversationId = rawConversationId ?? payloadConversationId
  const conversationId =
    selectedConversationId === null || selectedConversationId === undefined
      ? null
      : uuid(selectedConversationId)
  const type = boundedType(record.type)
  const occurredAt = field(record, 'occurredAt', 'occurred_at')
  const topLevelConversationId = uuid(rawConversationId)
  const nestedConversationId = uuid(payloadConversationId)

  if (
    !eventId ||
    !Number.isSafeInteger(eventCursor) ||
    (eventCursor as number) < 1 ||
    !Number.isSafeInteger(version) ||
    version !== REALTIME_ENVELOPE_VERSION ||
    !projectId ||
    (selectedConversationId !== null &&
      selectedConversationId !== undefined &&
      !conversationId) ||
    (rawConversationId !== null &&
      rawConversationId !== undefined &&
      payloadConversationId !== null &&
      payloadConversationId !== undefined &&
      topLevelConversationId !== nestedConversationId) ||
    !type ||
    typeof occurredAt !== 'string' ||
    occurredAt.length > 40 ||
    Number.isNaN(Date.parse(occurredAt)) ||
    !payload
  ) {
    return null
  }

  return {
    eventId,
    eventCursor: eventCursor as number,
    version: version as number,
    projectId,
    conversationId,
    type,
    occurredAt,
    payload,
  }
}

// M0/M1 provider stream events remain accepted while their durable event
// producers are migrated to the M2 outbox. They are deliberately excluded
// from the durable envelope contract and only drive ephemeral stream text.
export function parseLegacyRealtimeEvent(
  value: unknown,
): LegacyRealtimeEvent | null {
  const record = asRecord(value)
  if (
    record.version !== undefined ||
    record.eventCursor !== undefined ||
    record.event_cursor !== undefined
  ) {
    return null
  }
  const eventId = uuid(field(record, 'eventId', 'event_id'))
  const projectId = uuid(field(record, 'projectId', 'project_id'))
  const rawConversationId = field(
    record,
    'conversationId',
    'conversation_id',
  )
  const conversationId = rawConversationId ? uuid(rawConversationId) : null
  const type = boundedType(record.kind ?? record.type)
  const payload = boundedPayload(record.payload)
  if (!eventId || !projectId || (rawConversationId && !conversationId) || !type || !payload) {
    return null
  }
  return { eventId, projectId, conversationId, type, payload }
}

export function streamDelta(
  event: Pick<LegacyRealtimeEvent, 'type' | 'payload'>,
): { messageId: string; runId: string | null; delta: string } | null {
  if (event.type !== 'message.delta') return null
  const messageId = field(event.payload, 'messageId', 'message_id')
  const rawRunId = field(event.payload, 'agentRunId', 'agent_run_id')
  const runId = rawRunId === null || rawRunId === undefined ? null : uuid(rawRunId)
  const delta = event.payload.delta
  if (
    typeof messageId !== 'string' ||
    messageId.length === 0 ||
    messageId.length > 200 ||
    (rawRunId !== null && rawRunId !== undefined && !runId) ||
    typeof delta !== 'string' ||
    !isSafeMessageText(delta)
  ) {
    return null
  }
  return { messageId, runId, delta }
}

export function durableMessageId(
  event: Pick<RealtimeEnvelope, 'type' | 'payload'>,
): string | null {
  if (event.type !== 'conversation.message.created') return null
  return uuid(field(event.payload, 'messageId', 'message_id'))
}

/** A message that was edited or removed, from a conversation.message.updated event. */
export type MessageUpdate = {
  conversationId: string
  messageId: string
  revision: number
  removed: boolean
}

export function messageUpdate(
  event: Pick<RealtimeEnvelope, 'type' | 'payload' | 'conversationId'>,
): MessageUpdate | null {
  if (event.type !== 'conversation.message.updated') return null
  const conversationId = event.conversationId
  const messageId = uuid(field(event.payload, 'messageId', 'message_id'))
  const revision = event.payload.revision
  const removed = event.payload.removed
  if (
    !conversationId ||
    !messageId ||
    !Number.isSafeInteger(revision) ||
    (revision as number) < 1 ||
    typeof removed !== 'boolean'
  ) {
    return null
  }
  return { conversationId, messageId, revision: revision as number, removed }
}

export function terminalAgentRunId(
  event: Pick<RealtimeEnvelope, 'type' | 'payload'>,
): string | null {
  if (event.type !== 'conversation.agent_run.updated') return null
  const state = event.payload.state
  const terminal =
    state === 'completed' ||
    state === 'failed' ||
    state === 'canceled' ||
    state === 'interrupted'
  if (!terminal) return null
  return uuid(field(event.payload, 'agentRunId', 'agent_run_id'))
}

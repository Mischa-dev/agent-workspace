import { describe, expect, it } from 'vitest'

import {
  durableMessageId,
  parseLegacyRealtimeEvent,
  parseRealtimeEnvelope,
  streamDelta,
  terminalAgentRunId,
} from './realtimeModel'

const eventId = '019fb600-0000-7000-8000-000000000001'
const projectId = '019fb600-0000-7000-8000-000000000002'
const conversationId = '019fb600-0000-7000-8000-000000000003'

describe('realtime wire validation', () => {
  it('accepts the bounded version-one outbox envelope', () => {
    expect(
      parseRealtimeEnvelope({
        event_id: eventId,
        event_cursor: 17,
        version: 1,
        project_id: projectId,
        type: 'conversation.message.created',
        occurred_at: '2026-07-31T15:00:00.000000Z',
        payload: { conversationId, messageId: eventId },
      }),
    ).toMatchObject({
      eventId,
      eventCursor: 17,
      projectId,
      conversationId,
    })
  })

  it('rejects conflicting top-level and payload Conversation boundaries', () => {
    expect(
      parseRealtimeEnvelope({
        eventId,
        eventCursor: 1,
        version: 1,
        projectId,
        conversationId,
        type: 'conversation.message.created',
        occurredAt: '2026-07-31T15:00:00Z',
        payload: {
          conversationId: '019fb600-0000-7000-8000-000000000004',
        },
      }),
    ).toBeNull()
  })

  it('correlates only durable Message events to an ephemeral stream', () => {
    expect(
      durableMessageId({
        type: 'conversation.message.created',
        payload: { messageId: eventId },
      }),
    ).toBe(eventId)
    expect(
      durableMessageId({
        type: 'conversation.agent_run.updated',
        payload: { messageId: eventId },
      }),
    ).toBeNull()
    expect(
      durableMessageId({
        type: 'conversation.message.created',
        payload: { messageId: 'not-a-uuid' },
      }),
    ).toBeNull()
  })

  it('recognizes only terminal Agent Run invalidations as stream endings', () => {
    for (const state of ['completed', 'failed', 'canceled', 'interrupted']) {
      expect(
        terminalAgentRunId({
          type: 'conversation.agent_run.updated',
          payload: { state, agentRunId: eventId },
        }),
      ).toBe(eventId)
    }
    for (const state of [
      'created',
      'queued',
      'starting',
      'working',
      'blocked',
      'recovering',
    ]) {
      expect(
        terminalAgentRunId({
          type: 'conversation.agent_run.updated',
          payload: { state, agentRunId: eventId },
        }),
      ).toBeNull()
    }
    expect(
      terminalAgentRunId({
        type: 'conversation.message.created',
        payload: { state: 'failed', agentRunId: eventId },
      }),
    ).toBeNull()
  })

  it('rejects unknown versions, invalid identifiers, and oversized payloads', () => {
    const base = {
      eventId,
      eventCursor: 1,
      version: 1,
      projectId,
      type: 'conversation.created',
      occurredAt: '2026-07-31T15:00:00Z',
      payload: {},
    }
    expect(parseRealtimeEnvelope({ ...base, version: 2 })).toBeNull()
    expect(parseRealtimeEnvelope({ ...base, projectId: 'not-a-uuid' })).toBeNull()
    expect(
      parseRealtimeEnvelope({ ...base, payload: { value: 'x'.repeat(66_000) } }),
    ).toBeNull()
  })

  it('keeps legacy provider deltas ephemeral and bounded', () => {
    const event = parseLegacyRealtimeEvent({
      event_id: eventId,
      project_id: projectId,
      conversation_id: conversationId,
      kind: 'message.delta',
      payload: { message_id: 'provider-message', delta: 'hello' },
    })
    expect(event).not.toBeNull()
    expect(event && streamDelta(event)).toEqual({
      messageId: 'provider-message',
      runId: null,
      delta: 'hello',
    })
  })

  it('rejects unsafe and over-limit provider deltas', () => {
    const event = (delta: string) => ({
      type: 'message.delta',
      payload: { message_id: 'provider-message', delta },
    })
    expect(streamDelta(event('unsafe\u0000text'))).toBeNull()
    expect(streamDelta(event('🙂'.repeat(20_001)))).toBeNull()
    expect(streamDelta(event('🙂'.repeat(20_000)))).not.toBeNull()
  })
})

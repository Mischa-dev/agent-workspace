import assert from 'node:assert/strict'
import test from 'node:test'

import {
  MEASURED_SAMPLES,
  parseSetCookieHeaders,
  selectTimelineThread,
  summarizeDurations,
  validateLoopbackBaseUrl,
} from './private-host-performance-lib.mjs'

test('uses deterministic nearest-rank percentiles for 25 warm samples', () => {
  const values = Array.from({ length: MEASURED_SAMPLES }, (_, index) => 25 - index)
  assert.deepEqual(summarizeDurations(values), {
    samples: 25,
    minMs: 1,
    medianMs: 13,
    p95Ms: 24,
    maxMs: 25,
  })
  assert.throws(() => summarizeDurations(values.slice(1)), /exactly 25/)
})

test('accepts only explicit loopback origins', () => {
  assert.equal(validateLoopbackBaseUrl('http://127.0.0.1:3100'), 'http://127.0.0.1:3100')
  assert.equal(validateLoopbackBaseUrl('http://[::1]:3100'), 'http://[::1]:3100')
  assert.throws(() => validateLoopbackBaseUrl('https://example.com'), /loopback HTTP/)
  assert.throws(() => validateLoopbackBaseUrl('http://127.0.0.1:3100/path'), /origin-only/)
})

test('extracts cookie pairs without retaining attributes', () => {
  const cookies = parseSetCookieHeaders({
    getSetCookie: () => [
      'agent_workspace_session=session-value; HttpOnly; SameSite=Strict',
      'agent_workspace_csrf=csrf-value; SameSite=Strict',
    ],
  })
  assert.equal(cookies.get('agent_workspace_session'), 'session-value')
  assert.equal(cookies.get('agent_workspace_csrf'), 'csrf-value')
})

test('prefers a visible Thread with durable timeline entries', () => {
  const channels = [
    { threads: [{ id: 'empty-thread', highestSequence: 0 }] },
    { threads: [{ id: 'durable-thread', highestSequence: 10 }] },
  ]
  assert.equal(selectTimelineThread(channels)?.id, 'durable-thread')
  assert.equal(selectTimelineThread([{ threads: [{ id: 'fallback-thread' }] }])?.id, 'fallback-thread')
  assert.equal(selectTimelineThread([]), null)
})

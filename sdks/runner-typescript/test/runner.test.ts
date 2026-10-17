import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  DeliveryDeduper,
  McpClient,
  Runner,
  ToolError,
  WakeupError,
  parseSettings,
  signWakeup,
  stripLeadingMentions,
  verifyWakeup,
  type RequestContext,
} from '../src/index.ts'

const secret = 'test-webhook-secret'

function signed(body: object, timestamp = Math.floor(Date.now() / 1000)) {
  const text = JSON.stringify(body)
  return {
    body: text,
    headers: {
      'x-workspace-timestamp': String(timestamp),
      'x-workspace-signature': signWakeup(secret, String(timestamp), text),
      'x-workspace-delivery': (body as { deliveryId: string }).deliveryId,
    },
  }
}

test('a genuine wakeup verifies; a tampered, stale or unsigned one does not', () => {
  const wakeup = { event: 'reply.requested', deliveryId: 'd1', agentId: 'a', replyRequestId: 'r1' }
  const { body, headers } = signed(wakeup)
  assert.equal(verifyWakeup({ secret, headers, body }).replyRequestId, 'r1')
  assert.throws(() => verifyWakeup({ secret, headers, body: body.replace('r1', 'r2') }), WakeupError)
  assert.throws(() => verifyWakeup({ secret: 'other', headers, body }), WakeupError)
  const old = signed(wakeup, Math.floor(Date.now() / 1000) - 301)
  assert.throws(() => verifyWakeup({ secret, ...old }), /stale/)
  assert.throws(() => verifyWakeup({ secret, headers: {}, body }), /missing/)
  // The Fetch API's Headers work too.
  assert.equal(verifyWakeup({ secret, headers: new Headers(headers), body }).event, 'reply.requested')
})

test('deliveries are handled once', () => {
  const deduper = new DeliveryDeduper(2)
  assert.equal(deduper.first('a'), true)
  assert.equal(deduper.first('a'), false)
  deduper.first('b')
  deduper.first('c')
  assert.equal(deduper.first('a'), true, 'the oldest is forgotten past the limit')
})

test('launch settings come off the last line', () => {
  assert.deepEqual(parseSettings('Fix the flaky test\n\nSettings: model=fast-1 reasoning=high speed=standard'), {
    prompt: 'Fix the flaky test',
    settings: { model: 'fast-1', reasoning: 'high', speed: 'standard' },
  })
  assert.deepEqual(parseSettings('Just do it'), { prompt: 'Just do it', settings: {} })
  assert.equal(stripLeadingMentions('@reviewer, @codex: look at this'), 'look at this')
  assert.equal(stripLeadingMentions('please @codex look'), 'please @codex look')
})

type Call = { name: string; arguments: Record<string, unknown> }

/** A fake MCP endpoint: answers tools/call from `tools`, in JSON or SSE. */
function fakeMcp(tools: Record<string, (args: Record<string, unknown>) => unknown>, options: { sse?: boolean } = {}) {
  const calls: Call[] = []
  const sessions: (string | null)[] = []
  const fetchImpl = (async (_url: string, init: RequestInit) => {
    const message = JSON.parse(String(init.body)) as { id?: number; method: string; params?: { name: string; arguments: Record<string, unknown> } }
    const headers = init.headers as Record<string, string>
    assert.equal(headers.authorization, 'Bearer token-1')
    sessions.push(headers['mcp-session-id'] ?? null)
    if (message.id === undefined) return new Response(null, { status: 202 })
    let result: unknown = {}
    if (message.method === 'tools/call' && message.params) {
      calls.push(message.params)
      const handler = tools[message.params.name]
      const value = handler ? handler(message.params.arguments) : { outcome: 'not_found', message: 'no tool' }
      result = { content: [{ type: 'text', text: JSON.stringify(value) }], structuredContent: value }
    }
    const answer = JSON.stringify({ jsonrpc: '2.0', id: message.id, result })
    return options.sse
      ? new Response(`event: message\ndata: ${answer}\n\n`, { headers: { 'content-type': 'text/event-stream', 'mcp-session-id': 's1' } })
      : new Response(answer, { headers: { 'content-type': 'application/json', 'mcp-session-id': 's1' } })
  }) as typeof fetch
  return { fetchImpl, calls, sessions }
}

test('the MCP client initializes once, keeps its session, and reads JSON or SSE answers', async () => {
  for (const sse of [false, true]) {
    const { fetchImpl, calls, sessions } = fakeMcp({ inbox_list: () => ({ outcome: 'ok', value: { requests: [] } }) }, { sse })
    const client = new McpClient({ url: 'https://aw.example/api/v1/mcp', token: 'token-1', fetch: fetchImpl })
    assert.deepEqual(await client.callTool('inbox_list', {}), { outcome: 'ok', value: { requests: [] } })
    await client.callTool('inbox_list', {})
    assert.equal(calls.length, 2)
    assert.deepEqual(sessions, [null, 's1', 's1', 's1'], 'initialize, initialized, then two calls in the session')
  }
})

function claimFor(requestId: string, text: string, followsRequestId: string | null = null) {
  return {
    outcome: 'ok',
    value: {
      request: {
        requestId,
        projectId: 'p',
        conversationId: 'c',
        conversationKind: 'thread',
        conversationName: 'Fix',
        otherMembers: 'Mischa',
        messageId: `m-${requestId}`,
        messageAuthor: 'Mischa',
        messageExcerpt: text,
        required: true,
        mentioned: true,
        status: 'claimed',
        generation: 3,
        leaseExpiresAt: null,
        createdAt: 'now',
        replyConversationId: 'thread-1',
        threadId: 'thread-1',
        followsRequestId,
      },
      recentMessages: [{ messageId: `m-${requestId}`, author: 'Mischa', authorKind: 'human', text, createdAt: 'now' }],
      instructions: 'Reply.',
      session: null,
    },
  }
}

test('a wakeup is claimed, worked and answered; Stop aborts it; follow-ups join it', async () => {
  const { fetchImpl, calls } = fakeMcp({
    reply_claim: (args) =>
      args.requestId === 'r1' ? claimFor('r1', '@bot fix it\nSettings: model=m1') : claimFor(String(args.requestId), 'also this', 'r1'),
    session_bind: () => ({ outcome: 'ok', value: { sessionId: 's', conversationId: 'thread-1', externalRef: 'x', state: 'active' } }),
    reply_progress: () => ({ outcome: 'ok', value: { requestId: 'r1', status: 'claimed', leaseExpiresAt: 'later' } }),
    reply_send: () => ({ outcome: 'conflict', message: 'stopped' }),
  })
  let seen: RequestContext | undefined
  let followup: RequestContext | undefined
  let release!: () => void
  const finished = new Promise<void>((resolve) => (release = resolve))
  const runner = new Runner({
    mcpUrl: 'https://aw.example/api/v1/mcp',
    token: 'token-1',
    webhookSecret: secret,
    harness: 'custom',
    fetch: fetchImpl,
    onRequest: async (context) => {
      seen = context
      context.onFollowup((next) => (followup = next))
      await context.bindSession('session-9')
      await context.progress('Working')
      await new Promise((resolve) => context.signal.addEventListener('abort', resolve))
      await assert.rejects(context.reply('done'), (error: unknown) => error instanceof ToolError && error.outcome === 'conflict')
      release()
    },
  })
  const first = signed({ event: 'reply.requested', deliveryId: 'd1', agentId: 'a', replyRequestId: 'r1' })
  assert.equal(runner.receive(first.headers, first.body).status, 204)
  assert.equal(runner.receive(first.headers, first.body).status, 204, 'a retry is accepted and ignored')
  assert.equal(runner.receive({}, first.body).status, 401)
  while (!seen) await new Promise((resolve) => setTimeout(resolve, 5))
  assert.equal(seen.prompt, 'fix it')
  assert.deepEqual(seen.settings, { model: 'm1' })
  const more = signed({ event: 'reply.followup', deliveryId: 'd2', agentId: 'a', replyRequestId: 'r2', followsRequestId: 'r1' })
  runner.receive(more.headers, more.body)
  while (!followup) await new Promise((resolve) => setTimeout(resolve, 5))
  assert.equal(followup.prompt, 'also this')
  const stop = signed({ event: 'reply.cancelled', deliveryId: 'd3', agentId: 'a', replyRequestId: 'r1' })
  runner.receive(stop.headers, stop.body)
  await finished
  assert.equal(followup.signal.aborted, true, 'Stop reaches the follow-ups too')
  assert.equal(calls.filter((call) => call.name === 'reply_claim').length, 2)
  assert.deepEqual(calls.find((call) => call.name === 'session_bind')?.arguments, {
    conversationId: 'thread-1',
    externalRef: 'session-9',
    harness: 'custom',
  })
  assert.deepEqual(calls.find((call) => call.name === 'reply_progress')?.arguments, { requestId: 'r1', generation: 3, status: 'Working' })
})

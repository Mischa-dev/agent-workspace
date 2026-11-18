import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

import { WorkspaceBridge, type Turn, type TurnHandle } from '../src/bridge.ts'
import { CHANNEL, agentWorkspaceChannelPlugin, normalizeTarget, sendText, startAccount, type ResolvedAccount } from '../src/channel.ts'

const envelope = JSON.parse(readFileSync(new URL('../../../fixtures/delivery/envelope.json', import.meta.url), 'utf8'))
const CONVERSATION: string = envelope.data.conversationId
const MESSAGE: string = envelope.data.messageId
const ASKER: string = envelope.actor.accountId

let sequence = 0

/** The fixture's envelope as a `reply.*` event, the way the stream carries it. */
function replyEvent(type: string, requestId: string) {
  sequence++
  return {
    ...envelope,
    type,
    id: `evt-${sequence}`,
    cursor: `c-${sequence}`,
    reason: 'reply_required',
    data: { ...envelope.data, thing: { kind: 'reply_request', id: requestId } },
  }
}

class FakeWorkspace {
  calls: { name: string; args: Record<string, any> }[] = []
  readonly waiting: { requestId: string; status: string }[]
  constructor(waiting: { requestId: string; status: string }[] = []) {
    this.waiting = waiting
  }

  async callTool(name: string, args: Record<string, unknown>) {
    this.calls.push({ name, args: { ...args } })
    if (name === 'inbox_list') return ok({ requests: this.waiting })
    if (name === 'reply_claim') {
      return ok({
        request: {
          requestId: args.requestId,
          generation: 4,
          conversationId: CONVERSATION,
          replyConversationId: CONVERSATION,
          conversationKind: 'channel',
          conversationName: 'launch',
          messageId: MESSAGE,
          messageAuthor: 'Ada',
          messageExcerpt: '@helper what is left?',
          threadId: null,
        },
        recentMessages: [{ messageId: MESSAGE, author: 'Ada', authorId: ASKER, text: '@helper what is left?' }],
        instructions: '',
        session: null,
      })
    }
    if (name === 'reply_send' || name === 'message_send') return ok({ messageId: `m${this.calls.length}` })
    return ok({})
  }

  named(name: string) {
    return this.calls.filter((call) => call.name === name).map((call) => call.args)
  }
}

function ok(value: unknown) {
  return { outcome: 'ok', value }
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 10))

function bridgeWith(runTurn: (turn: Turn, handle: TurnHandle) => Promise<void>, allowFrom: string[] = []) {
  const workspace = new FakeWorkspace()
  return { workspace, bridge: new WorkspaceBridge({ tools: workspace, runTurn, allowFrom }) }
}

test('a request becomes one turn from the person who asked', async () => {
  const turns: Turn[] = []
  const { workspace, bridge } = bridgeWith(async (turn) => {
    turns.push(turn)
    await new Promise(() => {}) // still running
  })
  await bridge.hear(replyEvent('reply.requested', 'r1'))
  await bridge.hear(replyEvent('reply.requested', 'r1'))
  await bridge.hear({ ...envelope, id: 'not-a-reply' })

  assert.equal(workspace.named('reply_claim').length, 1)
  assert.equal(turns.length, 1)
  assert.deepEqual(
    { text: turns[0].text, chatType: turns[0].chatType, sender: turns[0].sender, conversationId: turns[0].conversationId },
    { text: 'what is left?', chatType: 'group', sender: { id: ASKER, name: 'Ada' }, conversationId: CONVERSATION },
  )
  assert.equal(bridge.answering(CONVERSATION), 'r1')
})

test('blocks go out as they come, the final closes the request, tools show as status', async () => {
  const { workspace, bridge } = bridgeWith(async (_turn, handle) => {
    await handle.typing()
    await handle.deliver('Reading the board', 'tool')
    await handle.deliver('Looking now.', 'block')
    await handle.deliver('Two tasks are left.', 'final')
    await handle.deliver('Also: one is blocked.', 'final')
    await handle.finish('completed')
  })
  await bridge.hear(replyEvent('reply.requested', 'r1'))
  await settle()

  assert.deepEqual(
    workspace.named('reply_send').map((a) => [a.text, a.final, a.requestId, a.generation]),
    [
      ['Looking now.', false, 'r1', 4],
      ['Two tasks are left.', true, 'r1', 4],
    ],
  )
  assert.deepEqual(workspace.named('message_send').map((a) => [a.conversationId, a.text]), [[CONVERSATION, 'Also: one is blocked.']])
  assert.deepEqual(workspace.named('reply_progress').map((a) => a.status), ['Working', 'Reading the board'])
  assert.deepEqual(workspace.named('reply_release'), [])
  assert.equal(bridge.answering(CONVERSATION), null)
})

test('a turn with no answer, a failure or a refusal gives the request back', async () => {
  const ends: ('completed' | 'failed' | 'declined')[] = ['completed', 'failed', 'declined']
  const { workspace, bridge } = bridgeWith(async (_turn, handle) => {
    await handle.finish(ends.shift()!)
  })
  for (const id of ['r1', 'r2', 'r3']) await bridge.hear(replyEvent('reply.requested', id))
  await settle()
  const { bridge: throwing, workspace: thrown } = bridgeWith(async () => {
    throw new Error('boom')
  })
  await throwing.hear(replyEvent('reply.requested', 'r4'))
  await settle()

  assert.deepEqual(
    workspace.named('reply_release').map((a) => a.requestId),
    ['r1', 'r2', 'r3'],
  )
  assert.equal(thrown.named('reply_release')[0].reason, 'The agent hit an error and stopped.')
})

test('Stop aborts the turn and nothing more is sent for it', async () => {
  let stopped: AbortSignal | undefined
  let late: (() => Promise<void>) | undefined
  const { workspace, bridge } = bridgeWith(async (_turn, handle) => {
    stopped = handle.stopped
    late = async () => {
      await handle.deliver('Too late', 'final')
      await handle.finish('completed')
    }
  })
  await bridge.hear(replyEvent('reply.requested', 'r1'))
  await bridge.hear(replyEvent('reply.cancelled', 'r1'))
  await late!()

  assert.equal(stopped!.aborted, true)
  assert.deepEqual(workspace.named('reply_send'), [])
  assert.deepEqual(workspace.named('message_send'), [])
  assert.deepEqual(workspace.named('reply_release'), [])
})

test('an allow-list gives back requests from anyone else', async () => {
  const turns: Turn[] = []
  const { workspace, bridge } = bridgeWith(async (turn) => void turns.push(turn), ['someone-else'])
  await bridge.hear(replyEvent('reply.requested', 'r1'))
  assert.equal(turns.length, 0)
  assert.deepEqual(workspace.named('reply_release').map((a) => a.requestId), ['r1'])
})

// ---------------------------------------------------------------------------
// The OpenClaw side, against a stand-in host runtime.

function account(overrides: Partial<ResolvedAccount> = {}): ResolvedAccount {
  return {
    accountId: 'default',
    enabled: true,
    configured: true,
    mcpUrl: 'https://workspace.example/api/v1/mcp',
    token: 'test-token',
    cursorFile: null,
    allowFrom: [],
    ...overrides,
  }
}

function hostRuntime(admission = 'dispatch') {
  const seen: { ingress?: any; context?: any; dispatch?: any } = {}
  return {
    seen,
    runtime: {
      inbound: {
        ingress: {
          resolveStable: async (params: Record<string, unknown>) => {
            seen.ingress = params
            return { ingress: { admission } }
          },
        },
        buildContext: (params: Record<string, unknown>) => {
          seen.context = params
          return { built: true }
        },
        dispatch: async (params: any) => {
          seen.dispatch = params
          await params.delivery.deliver({ text: 'Thinking it over' }, { kind: 'block' })
          await params.delivery.deliver({ text: 'Two tasks are left.' }, { kind: 'final' })
          return { admission: { kind: 'dispatch' }, dispatched: true, dispatchResult: { deferredToActiveRun: false } }
        },
      },
    },
  }
}

async function runAccount(admission = 'dispatch', waiting: { requestId: string; status: string }[] = []) {
  const workspace = new FakeWorkspace(waiting)
  const handlers: ((event: any) => unknown)[] = []
  const channel = {
    on: (_pattern: string, handler: (event: any) => unknown) => void handlers.push(handler),
    connect: ({ signal }: { signal?: AbortSignal }) => new Promise<void>((resolve) => signal?.addEventListener('abort', () => resolve())),
  }
  const statuses: Record<string, unknown>[] = []
  const abort = new AbortController()
  const { seen, runtime } = hostRuntime(admission)
  const running = startAccount(
    {
      cfg: {},
      accountId: 'default',
      account: account(),
      abortSignal: abort.signal,
      setStatus: (next) => void statuses.push(next),
      channelRuntime: runtime,
    },
    () => ({ tools: workspace, channel: channel as any }),
  )
  await settle()
  return { workspace, handlers, statuses, seen, abort, running }
}

test('a request runs as one OpenClaw turn and its final answers it', async () => {
  const { workspace, handlers, statuses, seen, abort, running } = await runAccount()
  await handlers[0](replyEvent('reply.requested', 'r1'))
  await settle()
  const sent = await sendText({ cfg: {}, to: `agent-workspace:${CONVERSATION}`, text: 'Daily summary' })
  abort.abort()
  await running

  assert.equal(seen.ingress.subject.stableId, ASKER)
  assert.equal(seen.ingress.dmPolicy, 'open')
  assert.equal(seen.context.message.bodyForAgent, 'what is left?')
  assert.equal(seen.context.extra.CommandInterpretationSuppressed, true)
  assert.equal(seen.dispatch.channel, CHANNEL)
  assert.ok(seen.dispatch.replyOptions.abortSignal instanceof AbortSignal)
  assert.deepEqual(
    workspace.named('reply_send').map((a) => [a.text, a.final]),
    [
      ['Thinking it over', false],
      ['Two tasks are left.', true],
    ],
  )
  assert.deepEqual(workspace.named('reply_release'), [])
  assert.ok(sent.messageId)
  assert.equal(workspace.named('message_send')[0].text, 'Daily summary')
  assert.deepEqual(statuses.map((s) => s.lifecycle), ['starting', 'ready', 'stopped'])
})

test('a turn the host turns away gives the request back, and waiting ones are picked up', async () => {
  const { workspace, abort, running, handlers } = await runAccount('drop', [
    { requestId: 'w1', status: 'pending' },
    { requestId: 'w2', status: 'answered' },
  ])
  await handlers[0](replyEvent('reply.requested', 'r1'))
  await settle()
  abort.abort()
  await running

  assert.deepEqual(workspace.named('reply_claim').map((a) => a.requestId), ['w1', 'r1'])
  assert.deepEqual(workspace.named('reply_release').map((a) => a.requestId).sort(), ['r1', 'w1'])
  assert.deepEqual(workspace.named('reply_send'), [])
})

test('the plugin, its manifest and its package agree', () => {
  const manifest = JSON.parse(readFileSync(new URL('../openclaw.plugin.json', import.meta.url), 'utf8'))
  const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'))
  assert.deepEqual(manifest.channels, [CHANNEL])
  assert.equal(pkg.openclaw.channel.id, CHANNEL)
  assert.equal((agentWorkspaceChannelPlugin as any).base.id, CHANNEL)
  assert.equal(normalizeTarget(`agent-workspace:${CONVERSATION.toUpperCase()}`), CONVERSATION)
  assert.equal(normalizeTarget('#general'), undefined)
})

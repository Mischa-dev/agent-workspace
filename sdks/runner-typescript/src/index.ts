// The runner side of Agent Workspace's agent runner contract
// (docs/agent-runner.md): verify a wakeup, claim the request, bind your
// session, report progress, post interim and final replies, and stop when a
// person presses Stop. No dependencies; Node 22 or later.

import { createHash, createHmac, timingSafeEqual } from 'node:crypto'
import { createServer, type IncomingHttpHeaders, type Server } from 'node:http'

// ---------------------------------------------------------------------------
// Wakeups

/** `ping` is a test wakeup from the Agents page: answer it and do nothing else. */
export type WakeupEvent = 'reply.requested' | 'reply.followup' | 'reply.cancelled' | 'inbox.available' | 'ping'

export type Wakeup = {
  event: WakeupEvent
  deliveryId: string
  agentId: string
  replyRequestId: string
  projectId?: string
  conversationId?: string
  threadId?: string | null
  messageId?: string
  generation?: number
  followsRequestId?: string | null
  sessionRef?: string | null
  sessionHarness?: string | null
}

export class WakeupError extends Error {
  readonly status: number
  constructor(message: string, status = 401) {
    super(message)
    this.name = 'WakeupError'
    this.status = status
  }
}

/** Five minutes, as docs/agent-runner.md asks. */
export const WAKEUP_TOLERANCE_SECONDS = 300

type HeaderBag = IncomingHttpHeaders | Headers | Record<string, string | string[] | undefined>

function header(headers: HeaderBag, name: string): string | undefined {
  if (typeof (headers as Headers).get === 'function') return (headers as Headers).get(name) ?? undefined
  const value = (headers as Record<string, string | string[] | undefined>)[name.toLowerCase()]
  return Array.isArray(value) ? value[0] : value
}

/** The signature Agent Workspace puts on a wakeup: `sha256=` and the hex HMAC of `"{timestamp}.{body}"`. */
export function signWakeup(secret: string, timestamp: string, body: string): string {
  return `sha256=${createHmac('sha256', secret).update(`${timestamp}.${body}`).digest('hex')}`
}

/**
 * Checks a wakeup's signature (in constant time) and age, and parses it.
 * Throws `WakeupError` for anything that isn't a genuine, fresh wakeup.
 */
export function verifyWakeup(options: {
  secret: string
  headers: HeaderBag
  body: string
  /** Unix seconds; defaults to now. */
  now?: number
  toleranceSeconds?: number
}): Wakeup {
  const timestamp = header(options.headers, 'x-workspace-timestamp')
  const signature = header(options.headers, 'x-workspace-signature')
  if (!timestamp || !signature || !/^\d{1,12}$/.test(timestamp)) throw new WakeupError('missing signature')
  const now = options.now ?? Math.floor(Date.now() / 1000)
  if (Math.abs(now - Number(timestamp)) > (options.toleranceSeconds ?? WAKEUP_TOLERANCE_SECONDS)) {
    throw new WakeupError('stale wakeup')
  }
  const expected = Buffer.from(signWakeup(options.secret, timestamp, options.body))
  const given = Buffer.from(signature)
  if (expected.length !== given.length || !timingSafeEqual(expected, given)) throw new WakeupError('bad signature')
  let wakeup: Wakeup
  try {
    wakeup = JSON.parse(options.body) as Wakeup
  } catch {
    throw new WakeupError('body is not JSON', 400)
  }
  if (typeof wakeup?.event !== 'string' || typeof wakeup.deliveryId !== 'string') {
    throw new WakeupError('not a wakeup', 400)
  }
  const delivery = header(options.headers, 'x-workspace-delivery')
  if (delivery && delivery !== wakeup.deliveryId) throw new WakeupError('delivery mismatch', 400)
  return wakeup
}

/** Remembers recent delivery IDs, so a retried wakeup is handled once. */
export class DeliveryDeduper {
  private readonly seen = new Map<string, true>()
  private readonly limit: number
  constructor(limit = 10_000) {
    this.limit = limit
  }

  /** True the first time an ID is seen. */
  first(deliveryId: string): boolean {
    if (this.seen.has(deliveryId)) return false
    this.seen.set(deliveryId, true)
    if (this.seen.size > this.limit) {
      const oldest = this.seen.keys().next().value
      if (oldest !== undefined) this.seen.delete(oldest)
    }
    return true
  }
}

// ---------------------------------------------------------------------------
// MCP over streamable HTTP

export class McpError extends Error {
  readonly code?: number
  constructor(message: string, code?: number) {
    super(message)
    this.name = 'McpError'
    this.code = code
  }
}

/** A reply tool answered with something other than `ok`. */
export class ToolError extends Error {
  readonly outcome: string
  readonly field?: string
  constructor(outcome: string, detail: { message?: string; field?: string; retryCategory?: string }) {
    super(detail.message ?? (detail.field ? `invalid ${detail.field}` : outcome))
    this.name = 'ToolError'
    this.outcome = outcome
    this.field = detail.field
  }

  /** `temporarily_unavailable`: worth retrying with backoff. */
  get retryable() {
    return this.outcome === 'temporarily_unavailable'
  }
}

type JsonRpcMessage = { jsonrpc: '2.0'; id?: number; method?: string; result?: unknown; error?: { code: number; message: string } }

const PROTOCOL_VERSION = '2025-06-18'

/** A small MCP client: `initialize`, then `tools/call`. JSON and SSE answers both work. */
export class McpClient {
  private readonly url: string
  private readonly token: string
  private readonly fetchImpl: typeof fetch
  private session: string | null = null
  private ready: Promise<void> | null = null
  private nextId = 1

  constructor(options: { url: string; token: string; fetch?: typeof fetch }) {
    this.url = options.url
    this.token = options.token
    this.fetchImpl = options.fetch ?? fetch
  }

  private async post(message: JsonRpcMessage): Promise<JsonRpcMessage | null> {
    const headers: Record<string, string> = {
      authorization: `Bearer ${this.token}`,
      'content-type': 'application/json',
      accept: 'application/json, text/event-stream',
    }
    if (this.session) {
      headers['mcp-session-id'] = this.session
      headers['mcp-protocol-version'] = PROTOCOL_VERSION
    }
    const response = await this.fetchImpl(this.url, { method: 'POST', headers, body: JSON.stringify(message) })
    const session = response.headers.get('mcp-session-id')
    if (session) this.session = session
    if (response.status === 404 && this.session && message.method !== 'initialize') {
      // The server forgot the session (a restart): start over once.
      this.session = null
      this.ready = null
      throw new McpError('session expired', 404)
    }
    if (!response.ok) throw new McpError(`MCP endpoint answered ${response.status}`, response.status)
    if (message.id === undefined) return null
    const text = await response.text()
    const type = response.headers.get('content-type') ?? ''
    const messages = type.includes('text/event-stream') ? parseSse(text) : [JSON.parse(text) as JsonRpcMessage]
    const answer = messages.find((candidate) => candidate.id === message.id)
    if (!answer) throw new McpError('no answer to the request')
    if (answer.error) throw new McpError(answer.error.message, answer.error.code)
    return answer
  }

  private initialize(): Promise<void> {
    this.ready ??= (async () => {
      await this.post({
        jsonrpc: '2.0',
        id: this.nextId++,
        method: 'initialize',
        params: {
          protocolVersion: PROTOCOL_VERSION,
          capabilities: {},
          clientInfo: { name: '@agent-workspace/runner', version: '0.1.0' },
        },
      } as JsonRpcMessage)
      await this.post({ jsonrpc: '2.0', method: 'notifications/initialized' })
    })().catch((error: unknown) => {
      this.ready = null
      throw error
    })
    return this.ready
  }

  /** Calls a tool and returns its structured result. */
  async callTool(name: string, args: Record<string, unknown>): Promise<unknown> {
    for (let attempt = 0; ; attempt++) {
      try {
        await this.initialize()
        const answer = await this.post({
          jsonrpc: '2.0',
          id: this.nextId++,
          method: 'tools/call',
          params: { name, arguments: args },
        } as JsonRpcMessage)
        return toolPayload(answer?.result)
      } catch (error) {
        if (attempt === 0 && error instanceof McpError && error.code === 404) continue
        throw error
      }
    }
  }
}

function parseSse(text: string): JsonRpcMessage[] {
  const messages: JsonRpcMessage[] = []
  for (const block of text.split(/\r?\n\r?\n/)) {
    const data = block
      .split(/\r?\n/)
      .filter((line) => line.startsWith('data:'))
      .map((line) => line.slice(5).trimStart())
      .join('\n')
    if (data) messages.push(JSON.parse(data) as JsonRpcMessage)
  }
  return messages
}

function toolPayload(result: unknown): unknown {
  const value = result as { structuredContent?: unknown; content?: { type: string; text?: string }[] } | undefined
  if (value?.structuredContent !== undefined) return value.structuredContent
  const text = value?.content?.find((part) => part.type === 'text')?.text
  if (text === undefined) return undefined
  try {
    return JSON.parse(text)
  } catch {
    return text
  }
}

// ---------------------------------------------------------------------------
// The reply tools

export type InboxItem = {
  requestId: string
  projectId: string
  conversationId: string
  conversationKind: string
  conversationName: string
  otherMembers: string
  messageId: string
  messageAuthor: string
  messageExcerpt: string
  required: boolean
  mentioned: boolean
  status: string
  generation: number
  leaseExpiresAt: string | null
  createdAt: string
  replyConversationId: string
  threadId: string | null
  followsRequestId: string | null
}

export type ContextMessage = {
  messageId: string
  author: string
  authorKind: string
  text: string
  createdAt: string
  removed?: boolean
  edited?: boolean
}

export type Claim = {
  request: InboxItem
  recentMessages: ContextMessage[]
  instructions: string
  session: { externalRef: string; harness: string; url: string | null; state: string } | null
  /** The run a launch opened for this request; the reply calls move it. */
  run?: { runId: string; state: string }
}

export type RunActivity = {
  /** Your own id for it: sending it again is a duplicate, never an error. */
  id: string
  kind: string
  summary?: string
  stepRef?: string
  parentRef?: string
  payload?: Record<string, unknown>
}

export type RunResult = { kind: string; url?: string; title?: string; branch?: string; number?: number }

function unwrap<T>(payload: unknown): T {
  const result = payload as { outcome?: string; value?: T; message?: string; field?: string; retryCategory?: string }
  if (result?.outcome === 'ok') return result.value as T
  throw new ToolError(result?.outcome ?? 'invalid_response', result ?? {})
}

/** The agent's reply tools, by their MCP names and arguments. */
export class ReplyTools {
  readonly mcp: McpClient
  constructor(mcp: McpClient) {
    this.mcp = mcp
  }

  async inboxList(limit?: number): Promise<InboxItem[]> {
    const value = unwrap<{ requests: InboxItem[] }>(await this.mcp.callTool('inbox_list', limit ? { limit } : {}))
    return value.requests
  }

  async claim(requestId: string): Promise<Claim> {
    return unwrap<Claim>(await this.mcp.callTool('reply_claim', { requestId }))
  }

  async sessionBind(options: {
    conversationId: string
    externalRef: string
    /** Optional: your launched run there shows it as its backend. */
    harness?: string
    url?: string
    state?: 'active' | 'idle'
  }) {
    return unwrap<{ sessionId: string; conversationId: string; externalRef: string; state: string }>(
      await this.mcp.callTool('session_bind', { ...options }),
    )
  }

  async progress(requestId: string, generation: number, status: string) {
    return unwrap<{ requestId: string; status: string; leaseExpiresAt: string }>(
      await this.mcp.callTool('reply_progress', { requestId, generation, status: status.slice(0, 200) }),
    )
  }

  /** `final: false` posts an interim message and keeps the claim. */
  async send(requestId: string, generation: number, text: string, options: { final?: boolean; clientRequestId?: string } = {}) {
    const args: Record<string, unknown> = { requestId, generation, text }
    if (options.final !== undefined) args.final = options.final
    if (options.clientRequestId) args.clientRequestId = options.clientRequestId
    return unwrap<{ requestId: string; status: string; messageId: string | null }>(await this.mcp.callTool('reply_send', args))
  }

  async pass(requestId: string, generation?: number, reason?: string) {
    return unwrap<{ requestId: string; status: string }>(
      await this.mcp.callTool('reply_pass', { requestId, ...(generation !== undefined && { generation }), ...(reason && { reason }) }),
    )
  }

  async release(requestId: string, reason: string, generation?: number) {
    return unwrap<{ requestId: string; status: string }>(
      await this.mcp.callTool('reply_release', { requestId, reason, ...(generation !== undefined && { generation }) }),
    )
  }

  /** Steps, tool calls, messages or a plan on a run (`claim.run`). */
  async runActivity(runId: string, activities: RunActivity[]) {
    return unwrap<Record<string, unknown>>(await this.mcp.callTool('run_activity_report', { runId, activities }))
  }

  /** Change a run's status line, title, link (`externalUrl`) or state. */
  async runUpdate(runId: string, change: Record<string, unknown>) {
    return unwrap<Record<string, unknown>>(await this.mcp.callTool('run_update', { runId, ...change }))
  }

  /** What a run produced: a pull request, branch, file or link. */
  async runResult(runId: string, results: RunResult[]) {
    return unwrap<Record<string, unknown>>(await this.mcp.callTool('run_result_report', { runId, results }))
  }
}

// ---------------------------------------------------------------------------
// Launch settings

export type LaunchSettings = { model?: string; reasoning?: string; speed?: string; [key: string]: string | undefined }

/**
 * Splits the `Settings: model=<id> reasoning=<level> speed=<id>` line a
 * launch from the Launch dialog puts last in its message from the prompt.
 */
export function parseSettings(text: string): { prompt: string; settings: LaunchSettings } {
  const lines = text.replace(/\s+$/, '').split('\n')
  const last = lines.at(-1)?.trim() ?? ''
  const match = /^Settings:\s*(.*)$/.exec(last)
  if (!match) return { prompt: text.trim(), settings: {} }
  const settings: LaunchSettings = {}
  for (const pair of match[1].split(/\s+/)) {
    const [key, ...rest] = pair.split('=')
    const value = rest.join('=')
    if (/^[a-zA-Z][\w-]{0,40}$/.test(key) && value) settings[key] = value
  }
  return { prompt: lines.slice(0, -1).join('\n').trim(), settings }
}

/** The message text without the @mentions it starts with. */
export function stripLeadingMentions(text: string): string {
  let rest = text.trim()
  while (rest.startsWith('@')) {
    const end = rest.search(/\s/)
    rest = (end < 0 ? '' : rest.slice(end)).replace(/^[\s,:]+/, '')
  }
  return rest.trim()
}

// ---------------------------------------------------------------------------
// The runner

export type RequestContext = {
  claim: Claim
  request: InboxItem
  /** What was asked, without leading @mentions or the settings line. */
  prompt: string
  settings: LaunchSettings
  /** The session already bound in this conversation, to resume. */
  sessionRef: string | null
  /** Aborted when a person presses Stop. */
  signal: AbortSignal
  progress(status: string): Promise<void>
  interim(text: string): Promise<void>
  reply(text: string): Promise<void>
  pass(reason?: string): Promise<void>
  release(reason: string): Promise<void>
  bindSession(externalRef: string, options?: { url?: string; state?: 'active' | 'idle' }): Promise<void>
  /** The run a launch opened for this request, or null. */
  runId: string | null
  /** One step of the work on the run (nothing without one). */
  step(title: string, stepId?: string): Promise<void>
  /** Something the run produced, such as a pull request (nothing without a run). */
  result(kind: string, url: string, title?: string): Promise<void>
  /** Follow-ups people send in the thread while this request runs. */
  onFollowup(listener: (followup: RequestContext) => void): void
}

export type RunnerOptions = {
  mcpUrl: string
  token: string
  webhookSecret?: string
  /** What your runner is, such as `claude_code` or `custom`, for session_bind. */
  harness: string
  onRequest: (context: RequestContext) => Promise<void>
  fetch?: typeof fetch
  onError?: (error: unknown) => void
}

type Active = { context: RequestContext; controller: AbortController; listeners: ((followup: RequestContext) => void)[]; queued: RequestContext[] }

export class Runner {
  readonly tools: ReplyTools
  private readonly options: RunnerOptions
  private readonly deduper = new DeliveryDeduper()
  private readonly active = new Map<string, Active>()
  /** Follow-ups already handed to the work running in their thread. */
  private readonly handedOff = new Set<string>()

  constructor(options: RunnerOptions) {
    this.options = options
    this.tools = new ReplyTools(new McpClient({ url: options.mcpUrl, token: options.token, fetch: options.fetch }))
  }

  /** Verifies and handles one wakeup. Answer the HTTP request with `status` right away. */
  receive(headers: HeaderBag, body: string): { status: number } {
    if (!this.options.webhookSecret) return { status: 500 }
    let wakeup: Wakeup
    try {
      wakeup = verifyWakeup({ secret: this.options.webhookSecret, headers, body })
    } catch (error) {
      return { status: error instanceof WakeupError ? error.status : 400 }
    }
    if (wakeup.event === 'ping') return { status: 204 }
    if (this.deduper.first(wakeup.deliveryId)) void this.dispatch(wakeup).catch((error) => this.report(error))
    return { status: 204 }
  }

  /** Handles a wakeup that is already verified (or a request found by polling). */
  async dispatch(wakeup: Pick<Wakeup, 'event' | 'replyRequestId'> & Partial<Wakeup>): Promise<void> {
    if (wakeup.event === 'reply.cancelled') {
      this.active.get(wakeup.replyRequestId)?.controller.abort(new Error('stopped by a person'))
      return
    }
    if (this.active.has(wakeup.replyRequestId) || this.handedOff.has(wakeup.replyRequestId)) return
    const claim = await this.tools.claim(wakeup.replyRequestId)
    const follows = claim.request.followsRequestId ?? wakeup.followsRequestId
    const parent = follows ? this.active.get(follows) : undefined
    const context = this.context(claim, wakeup.sessionRef ?? claim.session?.externalRef ?? null)
    if (parent) {
      // A follow-up joins the work already running in its thread.
      this.handedOff.add(claim.request.requestId)
      parent.controller.signal.addEventListener('abort', () => context.controller.abort(parent.controller.signal.reason), { once: true })
      if (parent.listeners.length) for (const listener of parent.listeners) listener(context.context)
      else parent.queued.push(context.context)
      return
    }
    this.active.set(claim.request.requestId, context)
    try {
      await this.options.onRequest(context.context)
    } catch (error) {
      if (!context.controller.signal.aborted) {
        await this.tools.release(claim.request.requestId, 'The runner hit an error and stopped.', claim.request.generation).catch(() => {})
        this.report(error)
      }
    } finally {
      this.active.delete(claim.request.requestId)
      for (const queued of context.queued) this.handedOff.delete(queued.request.requestId)
    }
  }

  private context(claim: Claim, sessionRef: string | null): Active {
    const request = claim.request
    const controller = new AbortController()
    const asked = claim.recentMessages.find((message) => message.messageId === request.messageId)?.text ?? request.messageExcerpt
    const { prompt, settings } = parseSettings(stripLeadingMentions(asked))
    const tools = this.tools
    const harness = this.options.harness
    const runId = claim.run?.runId ?? null
    const active: Active = {
      controller,
      listeners: [],
      queued: [],
      context: {
        claim,
        request,
        prompt,
        settings,
        sessionRef,
        signal: controller.signal,
        async progress(status) {
          await tools.progress(request.requestId, request.generation, status)
        },
        async interim(text) {
          await tools.send(request.requestId, request.generation, text, { final: false })
        },
        async reply(text) {
          await tools.send(request.requestId, request.generation, text)
        },
        async pass(reason) {
          await tools.pass(request.requestId, request.generation, reason)
        },
        async release(reason) {
          await tools.release(request.requestId, reason, request.generation)
        },
        async bindSession(externalRef, options = {}) {
          await tools.sessionBind({ conversationId: request.replyConversationId, externalRef, harness, ...options })
        },
        runId,
        async step(title, stepId) {
          if (!runId) return
          const id = stepId ?? `step:${createHash('sha256').update(title).digest('hex').slice(0, 16)}`
          await tools.runActivity(runId, [{ id, kind: 'step_started', summary: title.slice(0, 200), payload: { title: title.slice(0, 200) } }])
        },
        async result(kind, url, title) {
          if (!runId) return
          await tools.runResult(runId, [{ kind, url, ...(title && { title }) }])
        },
        onFollowup(listener) {
          active.listeners.push(listener)
          for (const queued of active.queued.splice(0)) listener(queued)
        },
      },
    }
    return active
  }

  /** Serves wakeups on `POST {path}`. */
  listen(port: number, options: { host?: string; path?: string } = {}): Server {
    const path = options.path ?? '/wake'
    const server = createServer((request, response) => {
      if (request.method !== 'POST' || request.url?.split('?')[0] !== path) {
        response.writeHead(404).end()
        return
      }
      const chunks: Buffer[] = []
      let size = 0
      request.on('data', (chunk: Buffer) => {
        size += chunk.length
        if (size > 1 << 20) request.destroy()
        else chunks.push(chunk)
      })
      request.on('end', () => {
        response.writeHead(this.receive(request.headers, Buffer.concat(chunks).toString('utf8')).status).end()
      })
    })
    server.listen(port, options.host ?? '127.0.0.1')
    return server
  }

  /** Without a public URL: polls `inbox_list` until the signal aborts. */
  async poll(options: { intervalMs?: number; signal?: AbortSignal } = {}): Promise<void> {
    const interval = options.intervalMs ?? 30_000
    while (!options.signal?.aborted) {
      try {
        for (const item of await this.tools.inboxList()) {
          // Claimed ones too: after a restart, claiming again picks them up.
          if (!['pending', 'claimed'].includes(item.status) || this.active.has(item.requestId)) continue
          void this.dispatch({ event: item.followsRequestId ? 'reply.followup' : 'reply.requested', replyRequestId: item.requestId }).catch((error) =>
            this.report(error),
          )
        }
      } catch (error) {
        this.report(error)
      }
      await new Promise((resolve) => setTimeout(resolve, interval))
    }
  }

  private report(error: unknown) {
    ;(this.options.onError ?? ((value: unknown) => console.error('agent runner:', value)))(error)
  }
}

/** Reads the runner settings Agent Workspace hands out (AGENT_WORKSPACE_*) from the environment. */
export function runnerSettingsFromEnv(env: Record<string, string | undefined> = process.env) {
  const need = (name: string) => {
    const value = env[name]
    if (!value) throw new Error(`${name} is not set`)
    return value
  }
  return {
    mcpUrl: need('AGENT_WORKSPACE_MCP_URL'),
    token: need('AGENT_WORKSPACE_TOKEN'),
    webhookSecret: env.AGENT_WORKSPACE_WEBHOOK_SECRET,
    handle: env.AGENT_WORKSPACE_HANDLE,
    harness: env.AGENT_WORKSPACE_RUNNER ?? 'custom',
  }
}

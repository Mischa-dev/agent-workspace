// The reference channel adapter (A2 3.10): everything an agent's chat
// integration needs from Agent Workspace, on the event log and the agent
// tools. Receive events (stream, falling back to long-poll, or a signed
// push), send and reply, edit, delete, react, show it's working, and fetch
// attached files. No dependencies; Node 22 or later.

import { createHmac, timingSafeEqual } from 'node:crypto'
import { readFileSync, renameSync, writeFileSync } from 'node:fs'

// ---------------------------------------------------------------------------
// Events

/** One event as every transport carries it (sdks/fixtures/delivery/envelope.json). */
export type WorkspaceEvent = {
  /** Such as `reply.requested` or `message.mentioned`; `ping` is a test push. */
  type: string
  timestamp: string
  /** The same on every transport and every retry: drop a repeat by it. */
  id: string
  /** Opaque; pass it back to read on from just after this event. Null on an older push. */
  cursor: string | null
  workspace?: { id: string; slug: string } | null
  recipient?: { accountId: string } | null
  /** Why it reached you, such as `mentioned` or `reply_required`. */
  reason?: string
  actor?: { accountId: string; kind: string } | null
  requestedBy?: { accountId: string; kind: string } | null
  /** Ids, kinds and field names only; fetch text live with your own access. */
  data: Record<string, unknown>
  /** Something you caused yourself. */
  self?: boolean
}

/** What an event is about, such as `{kind: 'reply_request', id}`. */
export function eventThing(event: WorkspaceEvent): { kind: string; id: string } | null {
  const thing = event.data?.thing as { kind?: unknown; id?: unknown } | undefined
  return typeof thing?.kind === 'string' && typeof thing.id === 'string' ? { kind: thing.kind, id: thing.id } : null
}

/** `reply.*` events: the reply request they are about. */
export function replyRequestId(event: WorkspaceEvent): string | null {
  const thing = eventThing(event)
  if (thing?.kind === 'reply_request') return thing.id
  return typeof event.data?.replyRequestId === 'string' ? event.data.replyRequestId : null
}

function matches(pattern: string, type: string): boolean {
  if (pattern === '*') return true
  if (pattern.endsWith('.*')) return type.startsWith(pattern.slice(0, -1))
  return pattern === type
}

// ---------------------------------------------------------------------------
// Pushes

export class PushError extends Error {
  readonly status: number
  constructor(message: string, status = 401) {
    super(message)
    this.name = 'PushError'
    this.status = status
  }
}

/** Five minutes either way, as the delivery contract asks. */
export const PUSH_TOLERANCE_SECONDS = 300

export type HeaderBag = Headers | Record<string, string | string[] | undefined>

export function header(headers: HeaderBag, name: string): string | undefined {
  if (typeof (headers as Headers).get === 'function') return (headers as Headers).get(name) ?? undefined
  const value = (headers as Record<string, string | string[] | undefined>)[name.toLowerCase()]
  return Array.isArray(value) ? value[0] : value
}

/** The Standard Webhooks signature: `v1,` and the base64 HMAC of `{id}.{timestamp}.{body}`. */
export function signStandard(secret: string, id: string, timestamp: string, body: string): string {
  const key = secret.startsWith('whsec_') ? Buffer.from(secret.slice(6), 'base64') : Buffer.from(secret)
  return `v1,${createHmac('sha256', key).update(`${id}.${timestamp}.${body}`).digest('base64')}`
}

/** The older signature: `sha256=` and the hex HMAC of `{timestamp}.{body}`, keyed by the whole secret. */
export function signLegacy(secret: string, timestamp: string, body: string): string {
  return `sha256=${createHmac('sha256', secret).update(`${timestamp}.${body}`).digest('hex')}`
}

function same(a: string, b: string): boolean {
  const left = Buffer.from(a)
  const right = Buffer.from(b)
  return left.length === right.length && timingSafeEqual(left, right)
}

/**
 * Checks a push (in constant time, against each secret, so a rotation's old
 * and new secret both work) and its age, and returns its event. A push to an
 * older `typed_v1` connection comes back in the same shape as any other
 * event. Throws `PushError` for anything that isn't a genuine, fresh push.
 */
export function verifyPush(options: {
  secret: string | string[]
  headers: HeaderBag
  body: string
  /** Unix seconds; defaults to now. */
  now?: number
  toleranceSeconds?: number
}): WorkspaceEvent {
  const secrets = Array.isArray(options.secret) ? options.secret : [options.secret]
  const now = options.now ?? Math.floor(Date.now() / 1000)
  const fresh = (timestamp: string) =>
    /^\d{1,12}$/.test(timestamp) && Math.abs(now - Number(timestamp)) <= (options.toleranceSeconds ?? PUSH_TOLERANCE_SECONDS)
  const id = header(options.headers, 'webhook-id')
  const timestamp = header(options.headers, 'webhook-timestamp') ?? header(options.headers, 'x-workspace-timestamp')
  const standard = header(options.headers, 'webhook-signature')
  const legacy = header(options.headers, 'x-workspace-signature')
  if (!timestamp || (!standard && !legacy)) throw new PushError('missing signature')
  if (!fresh(timestamp)) throw new PushError('stale push')
  let genuine = false
  for (const secret of secrets) {
    if (standard && id) {
      const expected = signStandard(secret, id, timestamp, options.body)
      genuine ||= standard.split(' ').some((given) => same(given, expected))
    } else if (legacy) {
      genuine ||= same(legacy, signLegacy(secret, timestamp, options.body))
    }
  }
  if (!genuine) throw new PushError('bad signature')
  let parsed: Record<string, unknown>
  try {
    parsed = JSON.parse(options.body) as Record<string, unknown>
  } catch {
    throw new PushError('body is not JSON', 400)
  }
  const event = typeof parsed?.type === 'string' ? (parsed as unknown as WorkspaceEvent) : fromLegacy(parsed)
  if (!event) throw new PushError('not an event', 400)
  if (id && id !== event.id) throw new PushError('delivery mismatch', 400)
  return event
}

/** An older `typed_v1` or `inbox_v1` wakeup, as an event. */
function fromLegacy(body: Record<string, unknown>): WorkspaceEvent | null {
  if (typeof body?.event !== 'string' || typeof body.deliveryId !== 'string') return null
  const type = body.event === 'inbox.available' ? 'reply.requested' : body.event
  const { event: _event, deliveryId, agentId, projectId, replyRequestId: request, ...rest } = body
  return {
    type,
    timestamp: new Date().toISOString(),
    id: deliveryId,
    cursor: null,
    recipient: typeof agentId === 'string' ? { accountId: agentId } : null,
    data: {
      ...rest,
      ...(typeof projectId === 'string' && { spaceId: projectId }),
      ...(typeof request === 'string' && { thing: { kind: 'reply_request', id: request } }),
    },
  }
}

/** Remembers recent event ids, so a repeat is handled once. */
export class Deduper {
  private readonly seen = new Map<string, true>()
  private readonly limit: number
  constructor(limit = 10_000) {
    this.limit = limit
  }

  /** True the first time an id is seen. */
  first(id: string): boolean {
    if (this.seen.has(id)) return false
    this.seen.set(id, true)
    if (this.seen.size > this.limit) {
      const oldest = this.seen.keys().next().value
      if (oldest !== undefined) this.seen.delete(oldest)
    }
    return true
  }
}

// ---------------------------------------------------------------------------
// Tools

/** Calls one of the agent tools and returns its structured result. */
export type ToolCaller = { callTool(name: string, args: Record<string, unknown>): Promise<unknown> }

/** A tool answered with something other than `ok`. */
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

export function unwrap<T>(payload: unknown): T {
  const result = payload as { outcome?: string; value?: T; message?: string; field?: string; retryCategory?: string }
  if (result?.outcome === 'ok') return result.value as T
  throw new ToolError(result?.outcome ?? 'invalid_response', result ?? {})
}

// ---------------------------------------------------------------------------
// Cursors

/** Where the channel keeps its place, so a restart reads on from there. */
export type CursorStore = { load(): string | null; save(cursor: string): void }

/** A cursor kept in one file, replaced whole on each save. */
export function fileCursor(path: string): CursorStore {
  return {
    load() {
      try {
        return readFileSync(path, 'utf8').trim() || null
      } catch {
        return null
      }
    },
    save(cursor) {
      writeFileSync(`${path}.tmp`, cursor)
      renameSync(`${path}.tmp`, path)
    },
  }
}

// ---------------------------------------------------------------------------
// The channel

export type ChannelState = 'connecting' | 'streaming' | 'polling' | 'offline' | 'closed'

export type ChannelOptions = {
  /** The server, such as `https://workspace.example`. */
  origin: string
  token: string
  /** The agent tools (an `McpClient`). */
  tools: ToolCaller
  /** One of the account's connections; else its default. */
  connection?: string
  /** Keep your place yourself; without one the server keeps it. */
  cursor?: CursorStore
  /** Long-poll only, for hosts that cut streams. */
  poll?: boolean
  fetch?: typeof fetch
  onState?: (state: ChannelState) => void
  onError?: (error: unknown) => void
}

type Handler = (event: WorkspaceEvent) => unknown | Promise<unknown>

/** One page of `GET /api/v1/events` (sdks/fixtures/delivery/poll-page.json). */
type Page = { events: WorkspaceEvent[]; nextCursor?: string; hasMore?: boolean }

export type SendOptions = {
  /** Answering a reply request: through `reply_send`, which closes it unless `final` is false. */
  reply?: { requestId: string; generation: number; final?: boolean }
  /** Your own id for the message; it comes back as `clientMessageId` on `message.created`. */
  clientMessageId?: string
  replyToMessageId?: string
  attachmentIds?: string[]
}

export class WorkspaceChannel {
  readonly tools: ToolCaller
  private readonly options: ChannelOptions
  private readonly fetchImpl: typeof fetch
  private readonly handlers: { pattern: string; handler: Handler }[] = []
  private readonly deduper = new Deduper()
  private place: string | null
  private delivered = 0

  constructor(options: ChannelOptions) {
    this.options = options
    this.tools = options.tools
    this.fetchImpl = options.fetch ?? fetch
    this.place = options.cursor?.load() ?? null
  }

  /** Handle events of a type, a family (`reply.*`) or every one (`*`). */
  on(pattern: string, handler: Handler): this {
    this.handlers.push({ pattern, handler })
    return this
  }

  /** The cursor after the last event handled. */
  get cursor(): string | null {
    return this.place
  }

  /** Remember that everything up to `cursor` is handled. */
  ack(cursor: string | null) {
    if (!cursor) return
    this.place = cursor
    this.options.cursor?.save(cursor)
  }

  /** Hands one event to its handlers, once, then moves the cursor past it. */
  async deliver(event: WorkspaceEvent): Promise<void> {
    this.delivered++
    if (this.deduper.first(event.id)) {
      for (const { pattern, handler } of this.handlers) {
        if (!matches(pattern, event.type)) continue
        try {
          await handler(event)
        } catch (error) {
          this.report(error)
        }
      }
    }
    this.ack(event.cursor)
  }

  /**
   * Checks a signed push and handles its event. Answer the HTTP request with
   * `status` right away; handling goes on after.
   */
  receivePush(secret: string | string[], headers: HeaderBag, body: string): { status: number; event?: WorkspaceEvent } {
    let event: WorkspaceEvent
    try {
      event = verifyPush({ secret, headers, body })
    } catch (error) {
      return { status: error instanceof PushError ? error.status : 400 }
    }
    if (event.type !== 'ping') void this.deliver({ ...event, cursor: null }).catch((error) => this.report(error))
    return { status: 204, event }
  }

  /**
   * Reads events until `signal` aborts: a stream, reconnecting from the
   * last cursor when it ends, or long-polling where a stream can't be had.
   * A refused credential ends it with the error.
   */
  async connect(options: { signal?: AbortSignal } = {}): Promise<void> {
    const signal = options.signal
    let streaming = !this.options.poll
    let failures = 0
    while (!signal?.aborted) {
      this.state('connecting')
      const started = Date.now()
      const before = this.delivered
      try {
        if (streaming) {
          const opened = await this.stream(signal)
          if (!opened) streaming = false
        } else {
          this.state('polling')
          await this.pollOnce(signal, 30)
        }
        failures = 0
        // A server that answers at once every time is not waited on in a
        // tight loop.
        if (this.delivered === before && Date.now() - started < 1000) await sleep(1000, signal)
      } catch (error) {
        if (signal?.aborted) break
        if (error instanceof ChannelRefused) {
          this.state('closed')
          throw error
        }
        this.report(error)
        this.state('offline')
        failures++
        await sleep(Math.min(30_000, 1000 * 2 ** Math.min(failures, 5)), signal)
      }
    }
    this.state('closed')
  }

  /** One page of events, waiting up to `waitSeconds` for the first; handles them. */
  async pollOnce(signal?: AbortSignal, waitSeconds = 0): Promise<WorkspaceEvent[]> {
    const url = this.eventsUrl({ wait: String(waitSeconds), ...(this.place && { cursor: this.place }) })
    const response = await this.fetchImpl(url, { headers: this.headers('application/json'), signal })
    await this.check(response)
    const page = (await response.json()) as Page
    await this.handlePage(page)
    return page.events
  }

  private async handlePage(page: Page) {
    for (const event of page.events ?? []) await this.deliver(event)
    this.ack(page.nextCursor ?? null)
  }

  /** False when the server doesn't stream here (fall back to polling). */
  private async stream(signal?: AbortSignal): Promise<boolean> {
    const headers = this.headers('text/event-stream')
    if (this.place) headers['last-event-id'] = this.place
    const response = await this.fetchImpl(this.eventsUrl({}), { headers, signal })
    await this.check(response)
    const type = response.headers.get('content-type') ?? ''
    if (type.includes('application/json')) {
      // Something on the way doesn't stream: this is a page, and reading
      // it may have moved the server's place, so handle it.
      await this.handlePage((await response.json()) as Page)
      return false
    }
    if (!type.includes('text/event-stream') || !response.body) {
      await response.body?.cancel().catch(() => {})
      return false
    }
    this.state('streaming')
    const reader = response.body.pipeThrough(new TextDecoderStream()).getReader()
    let buffer = ''
    for (;;) {
      const { value, done } = await reader.read()
      if (done) return true
      buffer += value
      let end: number
      while ((end = buffer.search(/\r?\n\r?\n/)) >= 0) {
        const block = buffer.slice(0, end)
        buffer = buffer.slice(end).replace(/^\r?\n\r?\n/, '')
        const event = parseSseBlock(block)
        if (event) await this.deliver(event)
      }
    }
  }

  private eventsUrl(query: Record<string, string>): string {
    const url = new URL('/api/v1/events', this.options.origin)
    for (const [key, value] of Object.entries(query)) url.searchParams.set(key, value)
    if (this.options.connection) url.searchParams.set('connection', this.options.connection)
    return url.toString()
  }

  private headers(accept: string): Record<string, string> {
    return { authorization: `Bearer ${this.options.token}`, accept }
  }

  private async check(response: Response) {
    if (response.ok) return
    const text = await response.text().catch(() => '')
    if (response.status === 401 || response.status === 403) throw new ChannelRefused(response.status, text)
    if (this.place && response.status < 500 && /cursor/.test(text)) {
      // The cursor is past retention (`cursor_expired`) or not this
      // connection's: read on from where the server kept this connection's
      // place, or from the oldest event kept.
      this.place = null
    }
    throw new Error(`events answered ${response.status}: ${text.slice(0, 200)}`)
  }

  // -------------------------------------------------------------------------
  // Talking

  /** Posts a message, or answers a reply request when `reply` is given. Returns the message's id. */
  async send(conversationId: string, text: string, options: SendOptions = {}): Promise<string | null> {
    if (options.reply) {
      const args: Record<string, unknown> = {
        requestId: options.reply.requestId,
        generation: options.reply.generation,
        text,
      }
      if (options.reply.final !== undefined) args.final = options.reply.final
      if (options.clientMessageId) args.clientRequestId = options.clientMessageId
      if (options.attachmentIds?.length) args.attachmentIds = options.attachmentIds
      const value = unwrap<{ messageId: string | null }>(await this.tools.callTool('reply_send', args))
      return value.messageId
    }
    const args: Record<string, unknown> = { conversationId, text }
    if (options.clientMessageId) args.clientRequestId = options.clientMessageId
    if (options.replyToMessageId) args.replyToMessageId = options.replyToMessageId
    if (options.attachmentIds?.length) args.attachmentIds = options.attachmentIds
    const value = unwrap<{ messageId?: string; message?: { id?: string } }>(await this.tools.callTool('message_send', args))
    return value.messageId ?? value.message?.id ?? null
  }

  async edit(messageId: string, text: string, clientRequestId?: string): Promise<void> {
    unwrap(await this.tools.callTool('message_edit', { messageId, text, ...(clientRequestId && { clientRequestId }) }))
  }

  async delete(messageId: string): Promise<void> {
    unwrap(await this.tools.callTool('message_delete', { messageId }))
  }

  async react(messageId: string, emoji: string, reacted = true): Promise<void> {
    unwrap(await this.tools.callTool('message_react', { messageId, emoji, reacted }))
  }

  /** Shows what you're doing on a reply request you hold (and keeps it held). */
  async typing(requestId: string, generation: number, status = 'Working'): Promise<void> {
    unwrap(await this.tools.callTool('reply_progress', { requestId, generation, status: status.slice(0, 200) }))
  }

  /** Fetches a file attached to a message or task, through a short-lived link. */
  async download(fileId: string): Promise<{ name: string; contentType: string; bytes: Uint8Array }> {
    const link = unwrap<{ url: string; name: string; contentType: string }>(await this.tools.callTool('file_link_get', { fileId }))
    const response = await this.fetchImpl(link.url)
    if (!response.ok) throw new Error(`the file answered ${response.status}`)
    return { name: link.name, contentType: link.contentType, bytes: new Uint8Array(await response.arrayBuffer()) }
  }

  /** Uploads a file to a conversation; pass its id in `attachmentIds`. */
  async upload(conversationId: string, name: string, bytes: Uint8Array, contentType?: string): Promise<string> {
    const value = unwrap<{ fileId?: string; id?: string; file?: { id: string } }>(
      await this.tools.callTool('file_upload', {
        conversationId,
        name,
        dataBase64: Buffer.from(bytes).toString('base64'),
        ...(contentType && { contentType }),
      }),
    )
    const id = value.fileId ?? value.id ?? value.file?.id
    if (!id) throw new ToolError('invalid_response', { message: 'file_upload returned no id' })
    return id
  }

  private state(state: ChannelState) {
    this.options.onState?.(state)
  }

  private report(error: unknown) {
    ;(this.options.onError ?? ((value: unknown) => console.error('agent workspace channel:', value)))(error)
  }
}

/** The server refused the credential: reconnecting won't help. */
export class ChannelRefused extends Error {
  readonly status: number
  constructor(status: number, body: string) {
    super(`the server refused the credential (${status}) ${body.slice(0, 200)}`.trim())
    this.name = 'ChannelRefused'
    this.status = status
  }
}

/** One SSE block as an event; comments and keep-alives give null. */
export function parseSseBlock(block: string): WorkspaceEvent | null {
  let id: string | null = null
  const data: string[] = []
  for (const line of block.split(/\r?\n/)) {
    if (line.startsWith(':')) continue
    const colon = line.indexOf(':')
    const field = colon < 0 ? line : line.slice(0, colon)
    const value = colon < 0 ? '' : line.slice(colon + 1).replace(/^ /, '')
    if (field === 'id') id = value
    else if (field === 'data') data.push(value)
  }
  if (!data.length) return null
  const event = JSON.parse(data.join('\n')) as WorkspaceEvent
  return { ...event, cursor: event.cursor ?? id }
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal?.aborted) return resolve()
    const timer = setTimeout(resolve, ms)
    signal?.addEventListener(
      'abort',
      () => {
        clearTimeout(timer)
        resolve()
      },
      { once: true },
    )
  })
}

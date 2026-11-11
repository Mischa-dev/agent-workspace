import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as Y from 'yjs'

import { ApiError } from '../../api/errors'
import { fromBase64, toBase64, type PageSyncReply } from './documentApi'
import { FROM_SERVER, PageSync, type PageSyncApi, type SaveFailure, type SaveState } from './pageSync'

/** A server that keeps a page's document the way apps/server/src/page_docs does. */
class FakeServer {
  doc = new Y.Doc()
  version = 1
  generation = 'first'
  sends = 0
  syncs = 0
  /** Set to make the next sends fail. */
  failWith: unknown[] = []

  constructor(text = 'Hello') {
    this.doc.clientID = 1
    this.doc.getText('t').insert(0, text)
  }

  reply(stateVector: Uint8Array): PageSyncReply {
    return {
      status: 'synced',
      generation: this.generation,
      version: this.version,
      update: toBase64(Y.encodeStateAsUpdate(this.doc, stateVector)),
      stateVector: toBase64(Y.encodeStateVector(this.doc)),
    }
  }

  api: PageSyncApi = {
    sync: async ({ generation, stateVector }) => {
      this.syncs += 1
      if (generation !== this.generation) return { status: 'replaced', generation: this.generation, version: this.version }
      return this.reply(fromBase64(stateVector))
    },
    send: async ({ generation, update, stateVector }) => {
      this.sends += 1
      const failure = this.failWith.shift()
      if (failure) throw failure
      if (generation !== this.generation) return { status: 'replaced', generation: this.generation, version: this.version }
      // Try it on a copy first: an update that builds on changes the server lacks waits.
      const copy = new Y.Doc()
      Y.applyUpdate(copy, Y.encodeStateAsUpdate(this.doc))
      Y.applyUpdate(copy, fromBase64(update))
      if (copy.store.pendingStructs || copy.store.pendingDs) {
        return { status: 'resync', generation, version: this.version, stateVector: toBase64(Y.encodeStateVector(this.doc)) }
      }
      const before = Y.encodeStateVector(this.doc)
      Y.applyUpdate(this.doc, fromBase64(update))
      if (toBase64(before) !== toBase64(Y.encodeStateVector(this.doc)) || update.length > 4) this.version += 1
      return this.reply(fromBase64(stateVector))
    },
  }

  /** Someone else's change, made on the server's copy. */
  change(edit: (text: Y.Text) => void) {
    edit(this.doc.getText('t'))
    this.version += 1
  }
}

type Client = { doc: Y.Doc; sync: PageSync; states: SaveState[]; failures: Array<SaveFailure | null>; replaced: number }

const clients: Client[] = []

function connect(server: FakeServer, options: { editable?: boolean } = {}): Client {
  const doc = new Y.Doc()
  Y.applyUpdate(doc, Y.encodeStateAsUpdate(server.doc), FROM_SERVER)
  const client: Client = { doc, states: [], failures: [], replaced: 0, sync: null as unknown as PageSync }
  client.sync = new PageSync({
    doc,
    generation: server.generation,
    version: server.version,
    api: server.api,
    editable: options.editable ?? true,
    onState: (state) => client.states.push(state),
    onFailure: (failure) => client.failures.push(failure),
    onReplaced: () => {
      client.replaced += 1
    },
  })
  clients.push(client)
  return client
}

const text = (doc: Y.Doc) => doc.getText('t').toString()

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  for (const client of clients.splice(0)) client.sync.destroy()
  vi.useRealTimers()
})

describe('a page kept in step with the server', () => {
  it('sends a change once typing pauses, then says it saved', async () => {
    const server = new FakeServer()
    const client = connect(server)
    client.doc.getText('t').insert(5, ' there')
    expect(client.sync.unsaved).toBe(true)
    expect(server.sends).toBe(0)
    await vi.advanceTimersByTimeAsync(400)
    expect(server.sends).toBe(1)
    expect(text(server.doc)).toBe('Hello there')
    expect(client.states).toEqual(['unsaved', 'saving', 'saved'])
    expect(client.sync.unsaved).toBe(false)
    expect(client.sync.currentVersion).toBe(2)
  })

  it('gathers typing into one request and sends what came in meanwhile next', async () => {
    const server = new FakeServer('')
    const client = connect(server)
    const typed = client.doc.getText('t')
    for (const char of 'abc') typed.insert(typed.length, char)
    await vi.advanceTimersByTimeAsync(400)
    expect(server.sends).toBe(1)
    typed.insert(3, 'd')
    typed.insert(4, 'e')
    await vi.advanceTimersByTimeAsync(400)
    expect(server.sends).toBe(2)
    expect(text(server.doc)).toBe('abcde')
  })

  it('brings in others’ changes when it hears of them, and not for its own', async () => {
    const server = new FakeServer()
    const client = connect(server)
    server.change((shared) => shared.insert(0, 'Oh, '))
    client.sync.heard(server.version)
    await vi.advanceTimersByTimeAsync(0)
    expect(text(client.doc)).toBe('Oh, Hello')
    expect(server.syncs).toBe(1)
    // An event for a version it already has fetches nothing.
    client.sync.heard(server.version)
    await vi.advanceTimersByTimeAsync(0)
    expect(server.syncs).toBe(1)
    // Its own change's event arrives after the reply.
    client.doc.getText('t').insert(0, '!')
    await vi.advanceTimersByTimeAsync(400)
    client.sync.heard(server.version)
    await vi.advanceTimersByTimeAsync(0)
    expect(server.syncs).toBe(1)
  })

  it('loses nothing when two people type at once', async () => {
    const server = new FakeServer('one two')
    const first = connect(server)
    const second = connect(server)
    first.doc.getText('t').insert(3, ' and a half')
    second.doc.getText('t').insert(7, ' three')
    second.doc.getText('t').delete(0, 1)
    await vi.advanceTimersByTimeAsync(400)
    first.sync.heard(server.version)
    second.sync.heard(server.version)
    await vi.advanceTimersByTimeAsync(0)
    expect(text(server.doc)).toBe('ne and a half two three')
    expect(text(first.doc)).toBe(text(server.doc))
    expect(text(second.doc)).toBe(text(server.doc))
  })

  it('sends everything again when the server lacks what a change built on', async () => {
    const server = new FakeServer()
    const client = connect(server)
    client.doc.getText('t').insert(5, '!')
    await vi.advanceTimersByTimeAsync(400)
    // The server loses that change, but keeps its generation.
    const older = new FakeServer()
    server.doc = older.doc
    client.doc.getText('t').insert(6, '?')
    await vi.advanceTimersByTimeAsync(400)
    expect(text(server.doc)).toBe('Hello!?')
    expect(client.states.at(-1)).toBe('saved')
  })

  it('loads the page again when its document was rebuilt', async () => {
    const server = new FakeServer()
    const client = connect(server)
    server.generation = 'second'
    client.doc.getText('t').insert(0, 'x')
    await vi.advanceTimersByTimeAsync(400)
    expect(client.replaced).toBe(1)
  })

  it('retries a failed send, then stops and says why', async () => {
    const server = new FakeServer()
    const client = connect(server)
    server.failWith = [new TypeError('offline'), new TypeError('offline')]
    client.doc.getText('t').insert(0, 'x')
    await vi.advanceTimersByTimeAsync(400)
    expect(client.states.at(-1)).toBe('error')
    await vi.advanceTimersByTimeAsync(2_000)
    expect(client.states.at(-1)).toBe('error')
    await vi.advanceTimersByTimeAsync(4_000)
    expect(text(server.doc)).toBe('xHello')
    expect(client.states.at(-1)).toBe('saved')
  })

  it('waits while offline instead of sending again at once, and sends when back online', async () => {
    const server = new FakeServer()
    const client = connect(server)
    const online = vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(false)
    try {
      server.failWith = Array.from({ length: 50 }, () => new TypeError('offline'))
      client.doc.getText('t').insert(0, 'x')
      await vi.advanceTimersByTimeAsync(400)
      expect(server.sends).toBe(1)
      expect(client.states.at(-1)).toBe('offline')
      // Typing while offline waits too.
      client.doc.getText('t').insert(0, 'y')
      await vi.advanceTimersByTimeAsync(1_000)
      expect(server.sends).toBe(1)
      // Still offline: one more try now and then, not a stream of them.
      await vi.advanceTimersByTimeAsync(10_000)
      expect(server.sends).toBe(2)
      online.mockReturnValue(true)
      server.failWith = []
      client.sync.resume()
      await vi.advanceTimersByTimeAsync(0)
      expect(server.sends).toBe(3)
      expect(text(server.doc)).toBe('yxHello')
      expect(client.states.at(-1)).toBe('saved')
    } finally {
      online.mockRestore()
    }
  })

  it('stops on a refused change until asked to try again', async () => {
    const server = new FakeServer()
    const client = connect(server)
    server.failWith = [new ApiError({ status: 400, code: 'validation_failed', message: 'That change can’t be saved.' })]
    client.doc.getText('t').insert(0, 'x')
    await vi.advanceTimersByTimeAsync(400)
    expect(client.states.at(-1)).toBe('failed')
    expect(client.failures.at(-1)?.message).toBe('That change can’t be saved.')
    // More typing doesn't send while it's stopped.
    client.doc.getText('t').insert(0, 'y')
    await vi.advanceTimersByTimeAsync(5_000)
    expect(server.sends).toBe(1)
    client.failures.at(-1)?.retry()
    await vi.advanceTimersByTimeAsync(0)
    expect(text(server.doc)).toBe('yxHello')
    expect(client.failures.at(-1)).toBeNull()
  })

  it('still sends what was typed when the page closes, then goes quiet', async () => {
    const server = new FakeServer()
    const client = connect(server)
    client.doc.getText('t').insert(5, '!')
    client.sync.destroy()
    await vi.advanceTimersByTimeAsync(0)
    expect(server.sends).toBe(1)
    expect(text(server.doc)).toBe('Hello!')
    // Nothing more is sent, fetched or told.
    const told = client.states.length
    client.doc.getText('t').insert(0, 'x')
    server.change((shared) => shared.insert(0, 'Oh, '))
    client.sync.heard(server.version)
    await vi.advanceTimersByTimeAsync(1_000)
    expect(server.sends).toBe(1)
    expect(server.syncs).toBe(0)
    expect(client.states.length).toBe(told)
  })

  it('only follows along when it may not edit', async () => {
    const server = new FakeServer()
    const client = connect(server, { editable: false })
    client.doc.getText('t').insert(0, 'x')
    await vi.advanceTimersByTimeAsync(1_000)
    expect(server.sends).toBe(0)
    server.change((shared) => shared.insert(5, '!'))
    client.sync.refresh()
    await vi.advanceTimersByTimeAsync(0)
    expect(text(client.doc)).toBe('xHello!')
  })
})

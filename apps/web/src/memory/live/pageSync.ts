import * as Y from 'yjs'

import { isApiError } from '../../api/errors'
import { describeError } from '../../ui/errorText'
import { fromBase64, toBase64, type PageSyncReply } from './documentApi'

// Keeps one browser's copy of a page's live document in step with the
// server (AWD-53). Changes made here go to the server a moment after typing
// stops, one request at a time; changes from others come back in the reply,
// or are fetched when the realtime channel says the page changed. The server
// merges everything, so nothing anyone typed is lost.

/** The server's changes arrive under this origin, so they aren't sent back. */
export const FROM_SERVER = 'page-sync:server'

export type SaveState = 'saved' | 'saving' | 'unsaved' | 'offline' | 'error' | 'failed'

export function saveLabel(state: SaveState) {
  switch (state) {
    case 'saved':
      return 'Saved'
    case 'saving':
      return 'Saving…'
    case 'unsaved':
      return 'Unsaved changes'
    case 'offline':
      return 'Offline: changes will save when you reconnect'
    case 'error':
      return 'Changes aren’t saving. Retrying…'
    case 'failed':
      return 'Changes aren’t saving'
  }
}

/** Why saving stopped, and how to try again. */
export type SaveFailure = { message: string; retry: () => void }

export type PageSyncApi = {
  sync: (body: { generation: string; stateVector: string }) => Promise<PageSyncReply>
  send: (body: { generation: string; update: string; stateVector: string }) => Promise<PageSyncReply>
}

export type PageSyncOptions = {
  /** The page's document, as the server sent it. */
  doc: Y.Doc
  generation: string
  version: number
  api: PageSyncApi
  /** Whether changes made here are sent (someone who may only read just follows along). */
  editable: boolean
  onState?: (state: SaveState) => void
  /** The page's version went up (a change here or anyone's). */
  onVersion?: (version: number) => void
  /** The page's document was rebuilt on the server: load it again. */
  onReplaced?: () => void
  /** Saving stopped for a reason waiting won't fix; null once it saves again. */
  onFailure?: (failure: SaveFailure | null) => void
  /** How long typing gathers before it's sent. */
  sendDelay?: number
  /** A failed send is tried again after this, then twice and three times this. */
  retryDelay?: number
}

const SEND_DELAY_MS = 400
const RETRY_MS = 2_000
const RETRIES = 3
/** While offline, a waiting change is tried this often until the browser says it's back. */
const OFFLINE_RETRY_MS = 10_000
/** Answers a retry can't change: the change itself was refused, or the page is gone. */
const REFUSALS = new Set([400, 401, 403, 404, 409, 413, 422])
/** A server that keeps saying "resync" has lost something; stop rather than loop. */
const MAX_RESYNCS = 3

export class PageSync {
  private readonly doc: Y.Doc
  private readonly options: PageSyncOptions
  private readonly generation: string
  /** The server's state vector, as last heard: what a change is sent against. */
  private serverVector: Uint8Array
  private version: number
  private wantSend = false
  private wantPull = false
  private sending = false
  private running = false
  private timer: ReturnType<typeof setTimeout> | undefined
  private failures = 0
  private resyncs = 0
  private blocked = false
  private stopped = false
  /** The page closed here: send what's waiting, then go quiet. */
  private closing = false
  private state: SaveState = 'saved'

  constructor(options: PageSyncOptions) {
    this.options = options
    this.doc = options.doc
    this.generation = options.generation
    this.version = options.version
    this.serverVector = Y.encodeStateVector(options.doc)
    options.doc.on('update', this.onUpdate)
  }

  /** Whether changes made here haven't reached the server yet. */
  get unsaved() {
    return this.wantSend || this.sending
  }

  get currentVersion() {
    return this.version
  }

  /**
   * Stops following the page and stops telling anyone about it. A change
   * that's waiting is sent now rather than lost (and retried if that fails),
   * so leaving a page right after typing keeps what was typed.
   */
  destroy() {
    if (this.closing) return
    this.closing = true
    this.wantPull = false
    this.doc.off('update', this.onUpdate)
    this.flush()
  }

  /** The realtime channel says the page is now at `version`. */
  heard(version: number) {
    if (version <= this.version || this.closing) return
    this.wantPull = true
    this.kick()
  }

  /** Fetch what changed, as after a reconnect or on a timer while the live connection is down. */
  refresh() {
    if (this.closing) return
    this.wantPull = true
    this.kick()
  }

  /** Send what's waiting now, as when the tab is closing or going to the background. */
  flush() {
    if (!this.wantSend || this.blocked) return
    clearTimeout(this.timer)
    this.timer = undefined
    this.kick()
  }

  /** Back online: try again what was waiting. */
  resume() {
    if (this.blocked || this.closing) return
    this.failures = 0
    this.flush()
    this.refresh()
  }

  /** Try again after saving stopped. */
  retry() {
    if (this.closing) return
    this.blocked = false
    this.failures = 0
    this.resyncs = 0
    this.options.onFailure?.(null)
    this.wantSend = this.wantSend || this.options.editable
    this.flush()
  }

  private readonly onUpdate = (_update: Uint8Array, origin: unknown) => {
    if (origin === FROM_SERVER || this.stopped || !this.options.editable) return
    this.wantSend = true
    if (this.blocked) return
    this.setState(navigator.onLine === false ? 'offline' : 'unsaved')
    this.schedule(this.options.sendDelay ?? SEND_DELAY_MS)
  }

  private setState(state: SaveState) {
    if (state === this.state) return
    this.state = state
    if (!this.closing) this.options.onState?.(state)
  }

  private schedule(delay: number) {
    if (this.timer !== undefined || this.stopped) return
    this.timer = setTimeout(() => {
      this.timer = undefined
      this.kick()
    }, delay)
  }

  private kick() {
    if (this.running || this.stopped) return
    void this.run()
  }

  /** One request at a time: a change once its delay is up, otherwise a fetch that's wanted. */
  private async run() {
    this.running = true
    try {
      while (!this.stopped) {
        if (this.wantSend && !this.blocked && this.timer === undefined) await this.send()
        else if (this.wantPull) await this.pull()
        else break
      }
    } finally {
      this.running = false
    }
  }

  private async send() {
    this.wantSend = false
    this.sending = true
    this.setState('saving')
    let reply: PageSyncReply
    try {
      reply = await this.options.api.send({
        generation: this.generation,
        update: toBase64(Y.encodeStateAsUpdate(this.doc, this.serverVector)),
        stateVector: toBase64(Y.encodeStateVector(this.doc)),
      })
    } catch (reason) {
      this.wantSend = true
      this.failed(reason)
      return
    } finally {
      this.sending = false
    }
    if (this.stopped) return
    if (this.failures > 0) {
      this.failures = 0
      if (!this.closing) this.options.onFailure?.(null)
    }
    if (reply.status === 'replaced') {
      this.replaced()
      return
    }
    if (reply.status === 'resync') {
      this.resyncs += 1
      if (reply.stateVector) this.serverVector = fromBase64(reply.stateVector)
      this.wantSend = true
      if (this.resyncs > MAX_RESYNCS) {
        this.stop('This page’s copy here and the saved one no longer fit together. Reload the page to carry on.')
      }
      return
    }
    this.resyncs = 0
    this.accept(reply)
    this.setState(this.wantSend ? 'unsaved' : 'saved')
  }

  private async pull() {
    this.wantPull = false
    let reply: PageSyncReply
    try {
      reply = await this.options.api.sync({ generation: this.generation, stateVector: toBase64(Y.encodeStateVector(this.doc)) })
    } catch {
      // The next event, reconnect or poll fetches it again.
      return
    }
    if (this.stopped) return
    if (reply.status === 'replaced') this.replaced()
    else this.accept(reply)
  }

  /** Takes in what the server sent back: what this copy lacked, and where the server is now. */
  private accept(reply: PageSyncReply) {
    if (reply.update) Y.applyUpdate(this.doc, fromBase64(reply.update), FROM_SERVER)
    if (reply.stateVector) this.serverVector = fromBase64(reply.stateVector)
    if (reply.version > this.version) {
      this.version = reply.version
      if (!this.closing) this.options.onVersion?.(reply.version)
    }
  }

  private replaced() {
    this.stopped = true
    if (!this.closing) this.options.onReplaced?.()
  }

  private failed(reason: unknown) {
    if (this.stopped) return
    if (isApiError(reason) && REFUSALS.has(reason.status)) {
      this.stop(describeError(reason).message)
      return
    }
    if (navigator.onLine === false) {
      // Coming back online resumes at once; until then, try again now and then
      // rather than straight away, which would send in a loop.
      this.setState('offline')
      this.schedule(OFFLINE_RETRY_MS)
      return
    }
    this.failures += 1
    if (this.failures > RETRIES) {
      this.stop(describeError(reason).message)
      return
    }
    this.setState('error')
    this.schedule((this.options.retryDelay ?? RETRY_MS) * this.failures)
  }

  /** Stops sending until someone asks to try again. */
  private stop(message: string) {
    this.blocked = true
    this.setState('failed')
    if (!this.closing) this.options.onFailure?.({ message, retry: () => this.retry() })
  }
}

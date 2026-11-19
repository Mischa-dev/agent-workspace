// Protocol 2 (W1.6, the server's K5.9): the live connection's frames on
// `/api/v1/realtime?v=2`. Every frame is an object with a `type`; a durable
// event carries its log position (a string, since positions can pass
// JavaScript's safe integers) around protocol 1's envelope, which the same
// handlers read. A server without protocol 2 sends bare envelopes, whose
// types all have a dot, so the two never collide.
//
// `apps/server/tests/fixtures/realtime_wire_v2.json` holds an example of every
// frame; `protocol.test.ts` parses it.

import { isSafeMessageText } from '../../conversations/messageContentModel'

/** A document a connection can watch: a page, sheet or whiteboard. */
export type DocumentRef = { kind: 'page' | 'sheet' | 'whiteboard'; id: string }

export type AccessSets = { workspaces: string[]; spaces: string[]; conversations: string[] }

export type ServerFrame =
  | { type: 'ready'; position: string; resumed: boolean; replayed: number }
  | { type: 'event'; position: string; event: unknown }
  | { type: 'access'; added: AccessSets; removed: AccessSets }
  | { type: 'resync'; reason: string; position: string | null }
  | { type: 'reconnect'; afterMs: number; reason: string }
  | { type: 'stream.delta'; conversationId: string; messageId: string; runId: string | null; delta: string }
  | { type: 'typing'; conversationId: string; accountId: string; until: number }
  | { type: 'presence'; workspaceId: string; accountId: string; online: boolean }
  | { type: 'awareness'; document: DocumentRef; connection: string; accountId: string; state: unknown }
  | { type: 'document.update'; document: DocumentRef; generation: string; version: number; update: string }
  /** A protocol 2 frame this client doesn't read (yet): ignored, never an envelope. */
  | { type: 'other' }

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
const POSITION = /^\d{1,20}$/
const DOCUMENT_KINDS = new Set(['page', 'sheet', 'whiteboard'])
/** Frame types of protocol 2 and of the live contract (docs/contracts/live.md). */
const FRAME_TYPES = new Set(['ready', 'event', 'access', 'resync', 'reconnect', 'ephemeral', 'error', 'ticket.stale', 'ticket.expired', 'ticket.accepted'])
const MAX_DELTA = 64_000
const MAX_RECONNECT_WAIT_MS = 60_000

const record = (value: unknown): Record<string, unknown> | null =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : null
const uuid = (value: unknown) => (typeof value === 'string' && UUID.test(value) ? value.toLowerCase() : null)
const position = (value: unknown) => {
  const text = typeof value === 'number' && Number.isSafeInteger(value) ? String(value) : value
  return typeof text === 'string' && POSITION.test(text) ? text : null
}
const ids = (value: unknown) => (Array.isArray(value) ? value.map(uuid).filter((id): id is string => id !== null) : [])
const sets = (value: unknown): AccessSets => {
  const fields = record(value) ?? {}
  return { workspaces: ids(fields.workspaces), spaces: ids(fields.spaces), conversations: ids(fields.conversations) }
}

function documentRef(value: unknown): DocumentRef | null {
  const fields = record(value)
  const id = uuid(fields?.id)
  return id && typeof fields?.kind === 'string' && DOCUMENT_KINDS.has(fields.kind) ? { kind: fields.kind as DocumentRef['kind'], id } : null
}

/** A position later than another; positions are decimal strings of any length. */
export function after(left: string, right: string | null | undefined) {
  if (!right) return true
  return left.length !== right.length ? left.length > right.length : left > right
}

/**
 * A protocol 2 frame, read and checked; null for anything that isn't one
 * (a protocol 1 envelope, or a malformed frame).
 */
export function parseServerFrame(wire: unknown): ServerFrame | null {
  const frame = record(wire)
  const type = frame?.type
  if (!frame || typeof type !== 'string' || !FRAME_TYPES.has(type)) return null
  switch (type) {
    case 'ready': {
      const at = position(frame.position)
      if (!at) return null
      return { type, position: at, resumed: frame.resumed === true, replayed: typeof frame.replayed === 'number' ? frame.replayed : 0 }
    }
    case 'event': {
      const at = position(frame.position)
      return at && record(frame.event) ? { type, position: at, event: frame.event } : null
    }
    case 'access':
      return { type, added: sets(frame.added), removed: sets(frame.removed) }
    case 'resync':
      return { type, reason: typeof frame.reason === 'string' ? frame.reason : 'resync', position: position(frame.position) }
    case 'reconnect': {
      const wait = typeof frame.afterMs === 'number' && Number.isFinite(frame.afterMs) ? frame.afterMs : 1_000
      return { type, afterMs: Math.min(Math.max(wait, 0), MAX_RECONNECT_WAIT_MS), reason: typeof frame.reason === 'string' ? frame.reason : 'reconnect' }
    }
    case 'ephemeral':
      return ephemeral(frame)
    default:
      return { type: 'other' }
  }
}

function ephemeral(frame: Record<string, unknown>): ServerFrame | null {
  switch (frame.kind) {
    case 'stream.delta': {
      const conversationId = uuid(frame.conversationId)
      const messageId = typeof frame.messageId === 'string' && frame.messageId.length > 0 && frame.messageId.length <= 200 ? frame.messageId : null
      const runId = frame.runId === null || frame.runId === undefined ? null : uuid(frame.runId)
      if (!conversationId || !messageId || typeof frame.delta !== 'string' || frame.delta.length > MAX_DELTA || !isSafeMessageText(frame.delta)) return null
      if (runId === null && frame.runId !== null && frame.runId !== undefined) return null
      return { type: 'stream.delta', conversationId, messageId, runId, delta: frame.delta }
    }
    case 'typing': {
      const conversationId = uuid(frame.conversationId)
      const accountId = uuid(frame.accountId)
      const until = typeof frame.until === 'string' ? Date.parse(frame.until) : Number.NaN
      return conversationId && accountId && Number.isFinite(until) ? { type: 'typing', conversationId, accountId, until } : null
    }
    case 'presence': {
      const workspaceId = uuid(frame.workspaceId)
      const accountId = uuid(frame.accountId)
      return workspaceId && accountId && typeof frame.online === 'boolean' ? { type: 'presence', workspaceId, accountId, online: frame.online } : null
    }
    case 'awareness': {
      const document = documentRef(frame.document)
      const connection = uuid(frame.connection)
      const accountId = uuid(frame.accountId)
      return document && connection && accountId ? { type: 'awareness', document, connection, accountId, state: frame.state ?? null } : null
    }
    case 'document.update': {
      const document = documentRef(frame.document)
      const generation = uuid(frame.generation)
      const version = frame.version
      if (!document || !generation || typeof version !== 'number' || !Number.isSafeInteger(version) || typeof frame.update !== 'string') return null
      return { type: 'document.update', document, generation, version, update: frame.update }
    }
    default:
      return { type: 'other' }
  }
}

// --- What the client sends ---------------------------------------------------

export const hello = (resumeFrom: string | null) =>
  JSON.stringify(resumeFrom ? { type: 'hello', protocol: 2, resumeFrom, client: 'web' } : { type: 'hello', protocol: 2, client: 'web' })
export const watchFrame = (document: DocumentRef) => JSON.stringify({ type: 'watch', document })
export const unwatchFrame = (document: DocumentRef) => JSON.stringify({ type: 'unwatch', document })
export const typingFrame = (conversationId: string) => JSON.stringify({ type: 'typing', conversationId })
export const awarenessFrame = (document: DocumentRef, state: unknown) => JSON.stringify({ type: 'awareness', document, state })

export const documentKey = (document: DocumentRef) => `${document.kind}.${document.id}`

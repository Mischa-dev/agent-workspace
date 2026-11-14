import type { EmbedDecl, ThingDecl } from '../../modules/contract'
import { DECLARED_EMBEDS, DECLARED_THINGS } from '../../pieces/declared'
import { listQuery, readListState } from '../../work-items/listModel'
import type { WorkItemFilters } from '../../work-items/types'

/**
 * What a module's embed shows in a page (AWD-53), from the embed's path: one
 * thing (a task, a page, a whiteboard, a thread) when the path is where that
 * kind of thing opens, as its module's manifest declares, or a project's
 * tasks when it's the task list with its filters. Each draws with the piece
 * its module's manifest names for it; anything else stays a link.
 */
export type ModuleEmbedTarget =
  | { kind: 'thing'; thing: string; id: string; decl: ThingDecl; embed: EmbedDecl }
  | { kind: 'tasks'; projectId: string; filters: WorkItemFilters; decl: ThingDecl; embed: EmbedDecl }

/** Kinds the server opens as things (`GET /api/v1/things/{kind}/{id}`). */
const OPENS = new Set(['task', 'page', 'whiteboard', 'thread'])

const TASK_LIST = /^\/projects\/([^/]+)\/work-items\/?$/

const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/** A thing's address as a pattern: `{id}` captures, other names match one part. */
function pattern(href: string) {
  const parts = href.split(/(\{[a-zA-Z]+\})/)
  return new RegExp(`^${parts.map((part) => (part === '{id}' ? '([^/]+)' : /^\{[a-zA-Z]+\}$/.test(part) ? '[^/]+' : escape(part))).join('')}/?$`)
}

function embedFor(thing: string, type: string) {
  return DECLARED_EMBEDS.find((entry) => entry.embed.thing === thing && entry.embed.piece.type === type)?.embed ?? null
}

function declOf(kind: string) {
  return DECLARED_THINGS.find((entry) => entry.decl.kind === kind)?.decl ?? null
}

export function moduleEmbedTarget(href: string): ModuleEmbedTarget | null {
  let url: URL
  try {
    url = new URL(href, 'https://app.invalid')
  } catch {
    return null
  }
  if (url.origin !== 'https://app.invalid') return null
  try {
    const list = TASK_LIST.exec(url.pathname)
    if (list) {
      // The task list's own address contract: its filters and sort go to the
      // server, and its view (list or board) picks the piece.
      const state = readListState(url.searchParams)
      const board = state.view === 'board' || url.searchParams.get('layout') === 'board'
      const decl = declOf('task')
      const embed = embedFor('task', board ? 'board' : 'list')
      if (!decl || !embed) return null
      return { kind: 'tasks', projectId: decodeURIComponent(list[1]), filters: listQuery(state), decl, embed }
    }
    for (const { decl } of DECLARED_THINGS) {
      if (!OPENS.has(decl.kind) || !decl.href.includes('{id}')) continue
      const match = pattern(decl.href).exec(url.pathname)
      if (!match) continue
      const embed = embedFor(decl.kind, 'card')
      if (!embed) return null
      return { kind: 'thing', thing: decl.kind, id: decodeURIComponent(match[1]), decl, embed }
    }
  } catch {
    return null
  }
  return null
}

/** An address pasted from this app, as the path an embed keeps. */
export function embedPath(address: string, origin = window.location.origin): string | null {
  const text = address.trim()
  if (!text) return null
  try {
    const url = new URL(text, origin)
    return url.origin === origin ? `${url.pathname}${url.search}` : null
  } catch {
    return null
  }
}

import { createElement, type ReactNode } from 'react'

import { ThingLink } from './ThingLink'

// Links to things in this app (AWD-58, C2.23): the addresses the modules'
// manifests give their things (`things[].href` in `modules/*.json`; a test
// keeps the two in step), today's and the workspace's (`/w/<ws>/...`). The
// server keeps the same ones as references (thing_references), and they are
// drawn as a chip with the thing's title and status, and under a message as
// a card, read live with the reader's access.

const UUID = '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}'
const IS_UUID = new RegExp(`^${UUID}$`)

/** In a space: the word in today's address, and in the workspace's. */
const SPACE_KINDS: Record<string, string> = {
  'work-items': 'task',
  tasks: 'task',
  pages: 'page',
  threads: 'thread',
  channels: 'channel',
  chat: 'channel',
  sheets: 'sheet',
  collections: 'collection',
  dashboards: 'dashboard',
}
const ROOT_KINDS: Record<string, string> = {
  whiteboards: 'whiteboard',
  messages: 'conversation',
  dm: 'conversation',
  runs: 'run',
}
const WORD = (kinds: Record<string, string>) => Object.keys(kinds).join('|')
const PATHS = [
  // /projects/<space>/<word>/<id>
  new RegExp(`^/projects/${UUID}/(${WORD(SPACE_KINDS)})/(${UUID})/?$`),
  // /w/<ws>/s/<space>/<word>/<id>
  new RegExp(`^/w/[^/]+/s/${UUID}/(${WORD(SPACE_KINDS)})/(${UUID})/?$`),
  // /<word>/<id> and /w/<ws>/<word>/<id>
  new RegExp(`^(?:/w/[^/]+)?/(${WORD(ROOT_KINDS)})/(${UUID})/?$`),
]
/** A space itself: /projects/<id>, /w/<ws>/s/<id>. */
const SPACE = new RegExp(`^(?:/projects|/w/[^/]+/s)/(${UUID})/?$`)

export type ThingAddress = { kind: string; id: string; path: string }

/** The thing an address in this app opens, or null for anything else. */
export function thingAt(href: string, origin = window.location.origin): ThingAddress | null {
  let url: URL
  try {
    url = new URL(href, origin)
  } catch {
    return null
  }
  if (url.origin !== origin) return null
  const path = `${url.pathname}${url.search}${url.hash}`
  const space = SPACE.exec(url.pathname)
  if (space) return { kind: 'project', id: space[1].toLowerCase(), path }
  const match = PATHS.map((pattern) => pattern.exec(url.pathname)).find(Boolean)
  if (!match) return null
  const kind = SPACE_KINDS[match[1]] ?? ROOT_KINDS[match[1]]
  if (!kind) return null
  // A thread in a DM or group opens at its parent's address; a record in its collection's.
  const thread = kind === 'conversation' ? url.searchParams.get('thread') : null
  if (thread && IS_UUID.test(thread)) return { kind: 'thread', id: thread.toLowerCase(), path }
  const record = kind === 'collection' ? url.searchParams.get('record') : null
  if (record && IS_UUID.test(record)) return { kind: 'record', id: record.toLowerCase(), path }
  return { kind, id: match[2].toLowerCase(), path }
}

/** For `Markdown`'s `renderLink`: a chip for a link to a thing, null for any other link. */
export function thingLink(href: string, children: ReactNode, bare: boolean): ReactNode {
  const address = thingAt(href)
  return address ? createElement(ThingLink, { address, bare, children }) : null
}

/** Absolute links, Markdown link targets and bare paths into the app. */
const CANDIDATES = /https?:\/\/[^\s<>()[\]"']+|\]\((\/[^)\s]+)\)|(?:^|[\s(])(\/(?:projects|w|whiteboards|messages)\/[^\s<>()[\]"']+)/g

/** The things a message's text links to, each once, in order, at most `limit`. */
export function thingsLinkedIn(text: string, limit = 3, origin = window.location.origin): ThingAddress[] {
  const found: ThingAddress[] = []
  for (const match of text.matchAll(CANDIDATES)) {
    const href = (match[1] ?? match[2] ?? match[0]).replace(/[.,;:!?]+$/, '')
    const address = thingAt(href, origin)
    if (address && !found.some((seen) => seen.id === address.id)) found.push(address)
    if (found.length >= limit) break
  }
  return found
}

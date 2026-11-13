import { createElement, type ReactNode } from 'react'

import { ThingLink } from './ThingLink'

// Links to things in this app (AWD-58): the same addresses the server keeps
// as references (migration 20261113060000_thing_references.sql), drawn as a
// chip with the thing's title and status.

const UUID = '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}'
const PATH = new RegExp(`^/(?:projects/${UUID}/(work-items|pages|threads|channels)|(whiteboards|messages))/(${UUID})/?$`)
const KINDS: Record<string, string> = {
  'work-items': 'task',
  pages: 'page',
  threads: 'thread',
  channels: 'channel',
  whiteboards: 'whiteboard',
  messages: 'conversation',
}

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
  const match = PATH.exec(url.pathname)
  if (!match) return null
  const kind = KINDS[match[1] ?? match[2] ?? '']
  if (!kind) return null
  return { kind, id: match[3].toLowerCase(), path: `${url.pathname}${url.search}${url.hash}` }
}

/** For `Markdown`'s `renderLink`: a chip for a link to a thing, null for any other link. */
export function thingLink(href: string, children: ReactNode, bare: boolean): ReactNode {
  const address = thingAt(href)
  return address ? createElement(ThingLink, { address, bare, children }) : null
}

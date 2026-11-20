/**
 * Comments on board elements (C1.31): what's selected, where the board is
 * scrolled, and where a thread about some elements shows as a pin.
 */

import type { CommentThreadView } from '../api/generated/operations'

/** The selected elements and the box around them, in board coordinates. */
export type BoardSelection = { ids: string[]; x: number; y: number; width: number; height: number }

/** How the board is scrolled and zoomed. */
export type BoardViewport = { scrollX: number; scrollY: number; zoom: number }

/** A thread's place on a board: the elements it is about, and where its pin sits. */
export type ElementsAnchor = { kind: 'elements'; elementIds: string[]; at: { x: number; y: number } }

type Box = { id: string; x: number; y: number; width: number; height: number; isDeleted?: boolean }

/** The box around the elements with these ids, or null when none is live. */
export function selectionBounds(elements: readonly Box[], ids: readonly string[]): BoardSelection | null {
  const wanted = new Set(ids)
  let left = Infinity
  let top = Infinity
  let right = -Infinity
  let bottom = -Infinity
  const found: string[] = []
  for (const element of elements) {
    if (element.isDeleted || !wanted.has(element.id)) continue
    found.push(element.id)
    // Lines and arrows can run left or up, so their size can be negative.
    const x2 = element.x + element.width
    const y2 = element.y + element.height
    left = Math.min(left, element.x, x2)
    top = Math.min(top, element.y, y2)
    right = Math.max(right, element.x, x2)
    bottom = Math.max(bottom, element.y, y2)
  }
  if (!found.length) return null
  return { ids: found.sort(), x: left, y: top, width: right - left, height: bottom - top }
}

/** A new thread about the selection, its pin at the selection's top right. */
export function anchorFor(selection: BoardSelection): ElementsAnchor {
  return { kind: 'elements', elementIds: selection.ids, at: { x: selection.x + selection.width, y: selection.y } }
}

/** A thread's anchor when it is about board elements. */
export function elementsAnchor(value: unknown): ElementsAnchor | null {
  if (!value || typeof value !== 'object') return null
  const anchor = value as Record<string, unknown>
  const at = anchor.at as Record<string, unknown> | undefined
  if (anchor.kind !== 'elements' || !Array.isArray(anchor.elementIds) || !at) return null
  if (typeof at.x !== 'number' || typeof at.y !== 'number') return null
  return { kind: 'elements', elementIds: anchor.elementIds.filter((id): id is string => typeof id === 'string'), at: { x: at.x, y: at.y } }
}

/** Where a board point shows inside the canvas, in pixels from its top left. */
export function toScreen(point: { x: number; y: number }, viewport: BoardViewport) {
  return { left: (point.x + viewport.scrollX) * viewport.zoom, top: (point.y + viewport.scrollY) * viewport.zoom }
}

/** Whether two id lists name the same elements. */
export function sameIds(a: readonly string[], b: readonly string[]) {
  return a.length === b.length && a.every((id, index) => id === b[index])
}

/** A thread about some elements, with where its pin sits. */
export type PinnedThread = { thread: CommentThreadView; anchor: ElementsAnchor }

/** The open threads about elements on the board, each with its anchor. */
export function pinnedThreads(threads: readonly CommentThreadView[]): PinnedThread[] {
  return threads.flatMap((thread) => {
    const anchor = elementsAnchor(thread.anchor)
    return anchor && thread.status.toLowerCase() !== 'resolved' ? [{ thread, anchor }] : []
  })
}


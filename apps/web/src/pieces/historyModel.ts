import type { HistoryEntry } from '../api/generated/operations'

/**
 * Who made a version, and how: "Rook through Phone, for Lena". A version
 * an agent made for someone names them, so a reader sees on whose request
 * the change came.
 */
export function madeBy(entry: HistoryEntry): string {
  const who = entry.actor ?? 'Someone'
  const via = entry.via ? ` through ${entry.via}` : ''
  const forWhom = entry.requestedBy && entry.requestedById !== entry.actorId ? `, for ${entry.requestedBy}` : ''
  return `${who}${via}${forWhom}`
}

/** What a version changed, in a few words: "title, body" or "12 cells". */
export function changedSummary(entry: HistoryEntry): string | null {
  if (!entry.changed.length) return null
  if (entry.changed.length > 4) return `${entry.changed.length} places`
  return entry.changed.join(', ')
}

// The suggested changes on a record or a task (T1.41), as the server keeps
// them: each field as it reads now and as proposed.
import { useQuery } from '@tanstack/react-query'

import { opKey, opQuery } from '../api/ops'

export type ProposableKind = 'record' | 'task'

/** One field a proposal sets: how it reads now and as proposed. */
export type ProposedField = { field: string; name: string; before: string; after: string }

export type FieldProposal = {
  id: string
  author: string | null
  authorId: string
  note: string | null
  createdAt: string
  fields: ProposedField[]
}

/** A stored value as a person reads it. */
export function shownValue(value: unknown): string {
  if (value === null || value === undefined || value === '') return 'empty'
  if (typeof value === 'string') return value
  if (typeof value === 'number') return String(value)
  if (typeof value === 'boolean') return value ? 'Yes' : 'No'
  if (Array.isArray(value)) return value.length ? value.map(shownValue).join(', ') : 'empty'
  if (typeof value === 'object') {
    const named = value as { name?: unknown; label?: unknown; title?: unknown }
    const name = named.name ?? named.label ?? named.title
    if (typeof name === 'string') return name
  }
  return JSON.stringify(value)
}

/**
 * The fields in a proposal's change. A record's entries say how each value
 * read (`before`, `after`); a task's keep the values (`was`, `is`) and, for
 * ids, their names (`wasShown`, `isShown`).
 */
export function proposedFields(change: unknown): ProposedField[] {
  const changes = (change as { changes?: unknown } | null)?.changes
  if (!Array.isArray(changes)) return []
  return changes.flatMap((raw: unknown) => {
    if (!raw || typeof raw !== 'object') return []
    const entry = raw as Record<string, unknown>
    const field = typeof entry.field === 'string' ? entry.field : ''
    const name = typeof entry.name === 'string' && entry.name ? entry.name : field
    if (!name) return []
    const before = 'wasShown' in entry ? entry.wasShown : 'before' in entry ? entry.before : entry.was
    const after = 'isShown' in entry ? entry.isShown : 'after' in entry ? entry.after : entry.is
    return [{ field, name, before: shownValue(before), after: shownValue(after) }]
  })
}

/** The query key of one thing's waiting proposals, to refresh after suggesting. */
export function proposalsKey(kind: ProposableKind, id: string) {
  return opKey('proposal_list', { thingKind: kind, thingId: id })
}

/** One thing's proposals still waiting, newest first. */
export function useFieldProposals(kind: ProposableKind, id: string, enabled: boolean) {
  const query = useQuery({ ...opQuery('proposal_list', { thingKind: kind, thingId: id }), enabled })
  const proposals: FieldProposal[] = (query.data?.proposals ?? [])
    .filter((proposal) => proposal.status === 'pending')
    .map((proposal) => ({
      id: proposal.id,
      author: proposal.author ?? null,
      authorId: proposal.authorId,
      note: proposal.note ?? null,
      createdAt: proposal.createdAt,
      fields: proposedFields(proposal.change),
    }))
  return proposals
}

// Proposals on a whiteboard (C1.11), as the board reads them: the waiting
// ones, the elements each sets, and the pictures of the change.
import { useQuery } from '@tanstack/react-query'

import { opQuery } from '../api/ops'

/** One element a proposal sets. */
export type ProposedElement = { id: string; type: string; action: 'add' | 'change' | 'remove'; text: string }

export type BoardProposal = {
  id: string
  author: string | null
  authorId: string
  note: string | null
  createdAt: string
  elements: ProposedElement[]
  /** The changed part of the board before and after, as SVG. */
  before: string | null
  after: string | null
}

function text(value: unknown) {
  return typeof value === 'string' ? value : ''
}

/** The elements in a proposal's change, as the server keeps them. */
export function proposedElements(change: unknown): ProposedElement[] {
  const elements = (change as { elements?: unknown } | null)?.elements
  if (!Array.isArray(elements)) return []
  return elements.flatMap((entry: unknown) => {
    const element = entry as Record<string, unknown>
    const action = element.action
    if (action !== 'add' && action !== 'change' && action !== 'remove') return []
    const after = (element.after ?? {}) as Record<string, unknown>
    const before = (element.before ?? {}) as Record<string, unknown>
    const words = text(after.originalText) || text(after.text) || text(before.originalText) || text(before.text)
    return [{ id: text(element.id), type: text(element.type), action, text: words }]
  })
}

/** A preview picture, as an image source; the SVG is the server's own drawing. */
export function previewSource(svg: string | null) {
  return svg ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}` : null
}

/** Words for one proposed element. */
export function describeElement(element: ProposedElement) {
  const kind = element.type === 'text' ? 'text' : element.type || 'element'
  const verb = element.action === 'add' ? 'Add' : element.action === 'remove' ? 'Remove' : 'Change'
  const words = element.text.trim().replace(/\s+/g, ' ')
  return words ? `${verb} ${kind} “${words.length > 60 ? `${words.slice(0, 59)}…` : words}”` : `${verb} ${kind}`
}

/** The board's proposals still waiting, newest first. */
export function useBoardProposals(boardId: string, enabled: boolean) {
  const query = useQuery({ ...opQuery('proposal_list', { thingKind: 'whiteboard', thingId: boardId }), enabled })
  const proposals: BoardProposal[] = (query.data?.proposals ?? [])
    .filter((proposal) => proposal.status === 'pending')
    .map((proposal) => {
      const preview = (proposal.preview ?? {}) as { before?: unknown; after?: unknown }
      return {
        id: proposal.id,
        author: proposal.author ?? null,
        authorId: proposal.authorId,
        note: proposal.note ?? null,
        createdAt: proposal.createdAt,
        elements: proposedElements(proposal.change),
        before: typeof preview.before === 'string' ? preview.before : null,
        after: typeof preview.after === 'string' ? preview.after : null,
      }
    })
  return proposals
}

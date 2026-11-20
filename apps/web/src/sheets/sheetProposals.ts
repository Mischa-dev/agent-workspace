// Proposals on a sheet (C1.11), as the page reads them: the waiting ones,
// their cells, and which cells of a tab they would set.
import { useQuery } from '@tanstack/react-query'

import { opQuery } from '../api/ops'
import { cellKey } from './sheetModel'

/**
 * A cell a proposal would set: the input it read and the one proposed.
 * `gone` when its row or column was deleted since.
 */
export type ProposedCell = { tabId: string; tab: string; row: number; column: number; before: string; after: string; gone: boolean }

export type SheetProposal = {
  id: string
  author: string | null
  authorId: string
  note: string | null
  createdAt: string
  cells: ProposedCell[]
}

function text(value: unknown) {
  return typeof value === 'string' ? value : ''
}

/** The cells in a proposal's change, as the server keeps them. */
export function proposedCells(change: unknown): ProposedCell[] {
  const cells = (change as { cells?: unknown } | null)?.cells
  if (!Array.isArray(cells)) return []
  return cells.flatMap((entry: unknown) => {
    const cell = entry as Record<string, unknown>
    const row = Number(cell.row)
    const column = Number(cell.column)
    if (!Number.isInteger(row) || !Number.isInteger(column)) return []
    return [{ tabId: text(cell.tabId), tab: text(cell.tab), row, column, before: text(cell.before), after: text(cell.after), gone: cell.gone === true }]
  })
}

/** The sheet's proposals still waiting, newest first. */
export function useSheetProposals(sheetId: string, enabled: boolean) {
  const query = useQuery({ ...opQuery('proposal_list', { thingKind: 'sheet', thingId: sheetId }), enabled })
  const proposals: SheetProposal[] = (query.data?.proposals ?? [])
    .filter((proposal) => proposal.status === 'pending')
    .map((proposal) => ({
      id: proposal.id,
      author: proposal.author ?? null,
      authorId: proposal.authorId,
      note: proposal.note ?? null,
      createdAt: proposal.createdAt,
      cells: proposedCells(proposal.change),
    }))
  return proposals
}

/** The cells of one tab that a waiting proposal would set, by cell key. */
export function proposedOn(proposals: SheetProposal[], tabId: string) {
  const marks = new Map<string, string>()
  for (const proposal of proposals) for (const cell of proposal.cells) if (cell.tabId === tabId && !cell.gone) marks.set(cellKey(cell.row, cell.column), cell.after)
  return marks
}

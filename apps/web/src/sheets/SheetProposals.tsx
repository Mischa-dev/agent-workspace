// Proposals on a sheet (C1.11): the cells someone whose role only suggests
// would set, waiting for someone who edits the sheet to accept them (all,
// or some) or reject them. The author can withdraw their own.
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { callOp, opPrefix } from '../api/ops'
import { Banner, Button, Checkbox, Dialog, describeError } from '../ui'
import { cellName } from './sheetModel'
import type { SheetProposal } from './sheetProposals'

/** Shows the waiting proposals, each with its cells to pick from. */
export function SheetProposalsDialog({
  canDecide,
  onClose,
  onDecided,
  proposals,
  userId,
}: {
  canDecide: boolean
  onClose: () => void
  onDecided: () => void
  proposals: SheetProposal[]
  userId: string
}) {
  const client = useQueryClient()
  const [skipped, setSkipped] = useState<Record<string, number[]>>({})
  const [busy, setBusy] = useState<string | null>(null)
  const [problem, setProblem] = useState<string | null>(null)

  const decide = async (proposal: SheetProposal, decision: 'accept' | 'reject' | 'withdraw') => {
    const left = skipped[proposal.id] ?? []
    const parts = decision === 'accept' && left.length ? proposal.cells.map((_, index) => index).filter((index) => !left.includes(index)) : undefined
    if (parts && !parts.length) {
      setProblem('Choose at least one cell to accept.')
      return
    }
    setBusy(`${proposal.id}:${decision}`)
    setProblem(null)
    try {
      await callOp('proposal_decide', { proposalId: proposal.id, decision, ...(parts ? { parts } : {}) })
      void client.invalidateQueries({ queryKey: opPrefix('proposal_list') })
      onDecided()
    } catch (error) {
      setProblem(describeError(error).message)
    } finally {
      setBusy(null)
    }
  }

  return (
    <Dialog
      description="Changes from people whose role suggests on this sheet. Accepting writes them as their author's; a cell changed since can't be accepted."
      footer={
        <Button onClick={onClose} variant="ghost">
          Close
        </Button>
      }
      onClose={onClose}
      open
      title="Proposed changes"
      width={620}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      {proposals.length === 0 ? <p className="aw-sheet__proposals-empty">Nothing is waiting.</p> : null}
      {proposals.map((proposal) => {
        const left = skipped[proposal.id] ?? []
        const mine = proposal.authorId === userId
        return (
          <section aria-label={`Proposal from ${proposal.author ?? 'someone'}`} className="aw-sheet__proposal" key={proposal.id}>
            <p className="aw-sheet__proposal-by">
              {proposal.author ?? 'Someone'} proposes {proposal.cells.length === 1 ? 'one cell' : `${proposal.cells.length} cells`}
              {proposal.note ? `: ${proposal.note}` : ''}
            </p>
            <ul className="aw-sheet__proposal-cells">
              {proposal.cells.map((cell, index) => {
                const label = `${cell.tab} ${cellName({ row: cell.row, column: cell.column })}${cell.gone ? ' (deleted since)' : ''}`
                const change = (
                  <span className="aw-sheet__proposal-change">
                    <span className="aw-sheet__proposal-cell">{label}</span> <del>{cell.before || 'empty'}</del> <ins>{cell.after || 'empty'}</ins>
                  </span>
                )
                return (
                  <li key={`${cell.tabId}:${cell.row}:${cell.column}`}>
                    {canDecide && proposal.cells.length > 1 ? (
                      <Checkbox
                        checked={!left.includes(index)}
                        label={change}
                        onChange={(checked) =>
                          setSkipped((current) => ({
                            ...current,
                            [proposal.id]: checked ? left.filter((entry) => entry !== index) : [...left, index],
                          }))
                        }
                      />
                    ) : (
                      change
                    )}
                  </li>
                )
              })}
            </ul>
            <div className="aw-sheet__proposal-actions">
              {canDecide ? (
                <>
                  <Button busy={busy === `${proposal.id}:accept`} disabled={busy !== null} onClick={() => void decide(proposal, 'accept')} size="sm" variant="primary">
                    {left.length ? `Accept ${proposal.cells.length - left.length}` : 'Accept'}
                  </Button>
                  <Button busy={busy === `${proposal.id}:reject`} disabled={busy !== null} onClick={() => void decide(proposal, 'reject')} size="sm">
                    Reject
                  </Button>
                </>
              ) : null}
              {mine ? (
                <Button busy={busy === `${proposal.id}:withdraw`} disabled={busy !== null} onClick={() => void decide(proposal, 'withdraw')} size="sm" variant="ghost">
                  Withdraw
                </Button>
              ) : null}
            </div>
          </section>
        )
      })}
    </Dialog>
  )
}

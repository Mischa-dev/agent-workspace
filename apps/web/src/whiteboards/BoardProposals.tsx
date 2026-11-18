// Proposals on a whiteboard (C1.11): the elements someone whose role only
// suggests would draw, change or remove, with a picture of the change,
// waiting for someone who edits the board to accept them (all, or some) or
// reject them. The author can withdraw their own.
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { callOp, opPrefix } from '../api/ops'
import { Banner, Button, Checkbox, Dialog, describeError } from '../ui'
import { describeElement, previewSource, type BoardProposal } from './proposalModel'

/** Shows the waiting proposals, each with its picture and elements to pick from. */
export function BoardProposalsDialog({
  canDecide,
  onClose,
  proposals,
  userId,
}: {
  canDecide: boolean
  onClose: () => void
  proposals: BoardProposal[]
  userId: string
}) {
  const client = useQueryClient()
  const [skipped, setSkipped] = useState<Record<string, number[]>>({})
  const [busy, setBusy] = useState<string | null>(null)
  const [problem, setProblem] = useState<string | null>(null)

  const decide = async (proposal: BoardProposal, decision: 'accept' | 'reject' | 'withdraw') => {
    const left = skipped[proposal.id] ?? []
    const parts = decision === 'accept' && left.length ? proposal.elements.map((_, index) => index).filter((index) => !left.includes(index)) : undefined
    if (parts && !parts.length) {
      setProblem('Choose at least one change to accept.')
      return
    }
    setBusy(`${proposal.id}:${decision}`)
    setProblem(null)
    try {
      await callOp('proposal_decide', { proposalId: proposal.id, decision, ...(parts ? { parts } : {}) })
      void client.invalidateQueries({ queryKey: opPrefix('proposal_list') })
    } catch (error) {
      setProblem(describeError(error).message)
    } finally {
      setBusy(null)
    }
  }

  return (
    <Dialog
      description="Changes from people whose role suggests on this board. Accepting draws them as their author’s; anything changed since can’t be accepted."
      footer={
        <Button onClick={onClose} variant="ghost">
          Close
        </Button>
      }
      onClose={onClose}
      open
      title="Proposed changes"
      width={720}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      {proposals.length === 0 ? <p className="aw-wb-proposals__empty">Nothing is waiting.</p> : null}
      {proposals.map((proposal) => {
        const left = skipped[proposal.id] ?? []
        const mine = proposal.authorId === userId
        const before = previewSource(proposal.before)
        const after = previewSource(proposal.after)
        const count = proposal.elements.length
        return (
          <section aria-label={`Proposal from ${proposal.author ?? 'someone'}`} className="aw-wb-proposal" key={proposal.id}>
            <p className="aw-wb-proposal__by">
              {proposal.author ?? 'Someone'} proposes {count === 1 ? 'one change' : `${count} changes`}
              {proposal.note ? `: ${proposal.note}` : ''}
            </p>
            {before || after ? (
              <div className="aw-wb-proposal__pictures">
                <figure>
                  {before ? <img alt="The changed part of the board now" src={before} /> : <span className="aw-wb-proposal__nothing">Nothing yet</span>}
                  <figcaption>Now</figcaption>
                </figure>
                <figure>
                  {after ? <img alt="The changed part of the board as proposed" src={after} /> : <span className="aw-wb-proposal__nothing">Removed</span>}
                  <figcaption>Proposed</figcaption>
                </figure>
              </div>
            ) : null}
            <ul className="aw-wb-proposal__elements">
              {proposal.elements.map((element, index) => {
                const label = describeElement(element)
                return (
                  <li key={element.id}>
                    {canDecide && count > 1 ? (
                      <Checkbox
                        checked={!left.includes(index)}
                        label={label}
                        onChange={(checked) =>
                          setSkipped((current) => ({
                            ...current,
                            [proposal.id]: checked ? left.filter((entry) => entry !== index) : [...left, index],
                          }))
                        }
                      />
                    ) : (
                      label
                    )}
                  </li>
                )
              })}
            </ul>
            <div className="aw-wb-proposal__actions">
              {canDecide ? (
                <>
                  <Button busy={busy === `${proposal.id}:accept`} disabled={busy !== null} onClick={() => void decide(proposal, 'accept')} size="sm" variant="primary">
                    {left.length ? `Accept ${count - left.length}` : 'Accept'}
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

// Changes someone whose role only suggests made to a record or a task
// (T1.41), shown on the thing itself: each field as it reads now and as
// proposed, for someone who edits it to accept or reject, and for the
// author to withdraw. The request card in the inbox opens here.
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { callOp, ranOrThrow } from '../api/ops'
import { Banner, Button, describeError, formatFullDate, formatRelative } from '../ui'
import { proposalsKey, useFieldProposals, type FieldProposal, type ProposableKind } from './proposedChanges'
import './proposed.css'

/**
 * The waiting proposals on a record or task. Shows nothing while none wait.
 * `canDecide`: the viewer edits the thing. `onDecided` refreshes the thing
 * once an accepted proposal has changed it.
 */
export function ProposedChanges({
  canDecide,
  enabled = true,
  kind,
  onDecided,
  thingId,
  viewerId,
}: {
  canDecide: boolean
  enabled?: boolean
  kind: ProposableKind
  onDecided: () => void
  thingId: string
  viewerId: string
}) {
  const client = useQueryClient()
  const proposals = useFieldProposals(kind, thingId, enabled)
  const [busy, setBusy] = useState<string | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  if (!proposals.length) return null

  const decide = async (proposal: FieldProposal, decision: 'accept' | 'reject' | 'withdraw') => {
    setBusy(`${proposal.id}:${decision}`)
    setProblem(null)
    try {
      // A decision a review guard holds hasn't happened: say so instead of looking done.
      ranOrThrow(await callOp('proposal_decide', { proposalId: proposal.id, decision }), 'The decision')
      await client.invalidateQueries({ queryKey: proposalsKey(kind, thingId) })
      if (decision === 'accept') onDecided()
    } catch (error) {
      setProblem(describeError(error).message)
    } finally {
      setBusy(null)
    }
  }

  return (
    <section aria-labelledby={`proposed-${thingId}`} className="aw-proposed">
      <h3 className="aw-proposed__heading" id={`proposed-${thingId}`}>
        {proposals.length === 1 ? 'Suggested change' : `${proposals.length} suggested changes`}
      </h3>
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      {proposals.map((proposal) => {
        const mine = proposal.authorId === viewerId
        return (
          <article aria-label={`Suggestion from ${proposal.author ?? 'someone'}`} className="aw-proposed__item" key={proposal.id}>
            <p className="aw-proposed__by">
              <strong>{mine ? 'You' : (proposal.author ?? 'Someone')}</strong> suggested ·{' '}
              <time dateTime={proposal.createdAt} title={formatFullDate(proposal.createdAt)}>
                {formatRelative(proposal.createdAt)}
              </time>
            </p>
            {proposal.note ? <p className="aw-proposed__note">{proposal.note}</p> : null}
            <ul className="aw-proposed__fields">
              {proposal.fields.map((field) => (
                <li key={field.field || field.name}>
                  <span className="aw-proposed__name">{field.name}</span> <del>{field.before}</del> <span aria-hidden="true">→</span> <ins>{field.after}</ins>
                </li>
              ))}
            </ul>
            {canDecide || mine ? (
              <div className="aw-proposed__actions">
                {canDecide ? (
                  <>
                    <Button busy={busy === `${proposal.id}:accept`} disabled={busy !== null} onClick={() => void decide(proposal, 'accept')} size="sm" variant="primary">
                      Accept
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
            ) : null}
          </article>
        )
      })}
    </section>
  )
}

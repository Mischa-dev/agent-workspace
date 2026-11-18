import { here } from '../app/href'
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'

import { toastError } from '../app/toastError'
import { Banner, Button, showToast } from '../ui'
import { standingNote, useAnswerRequest, useSetBlock, type DmStanding } from './dmRules'

/**
 * C2.39: what stands between you and sending in a DM, above the composer:
 * a message request to you (accept or decline), your own request waiting,
 * someone you blocked (unblock), or no shared workspace any more.
 */
export function DmStandingBar({ conversationId, name, standing }: { conversationId: string; name: string; standing: DmStanding }) {
  const answer = useAnswerRequest()
  const setBlock = useSetBlock()
  const navigate = useNavigate()
  const [busy, setBusy] = useState<'accept' | 'decline' | 'unblock' | null>(null)
  const incoming = standing.request?.direction === 'incoming' && standing.request.state === 'pending'

  if (incoming) {
    const respond = (accept: boolean) => {
      setBusy(accept ? 'accept' : 'decline')
      answer(conversationId, accept)
        .then(() => {
          if (accept) return
          showToast({ tone: 'success', title: 'Request declined', description: `${name} isn’t told.` })
          navigate(here('/messages'))
        })
        .catch((reason: unknown) => toastError('The request wasn’t answered', reason))
        .finally(() => setBusy(null))
    }
    return (
      <Banner
        action={
          <span className="aw-dm-standing__actions">
            <Button busy={busy === 'accept'} disabled={busy !== null} onClick={() => respond(true)} size="sm" variant="primary">
              Accept
            </Button>
            <Button busy={busy === 'decline'} disabled={busy !== null} onClick={() => respond(false)} size="sm">
              Decline
            </Button>
          </span>
        }
        className="aw-conv__banner"
        title={`${name} wants to message you`}
      >
        Accept to see what else they send. Declining takes this conversation out of your list, and they aren’t told.
      </Banner>
    )
  }

  const note = standingNote(standing, name)
  if (!note) return null
  const unblock = () => {
    if (!standing.otherAccountId) return
    setBusy('unblock')
    setBlock(standing.otherAccountId, false)
      .then(() => showToast({ tone: 'success', title: `${name} unblocked` }))
      .catch((reason: unknown) => toastError('They weren’t unblocked', reason))
      .finally(() => setBusy(null))
  }
  return (
    <Banner
      action={
        standing.reason === 'blocked' ? (
          <Button busy={busy === 'unblock'} onClick={unblock} size="sm">
            Unblock
          </Button>
        ) : undefined
      }
      className="aw-conv__banner"
      role="note"
      tone={standing.reason === 'waiting' ? 'info' : 'warning'}
    >
      {note}
    </Banner>
  )
}

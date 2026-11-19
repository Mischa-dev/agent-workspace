import { useState } from 'react'

import { toastError } from '../app/mutations'
import { useWorkspaceId } from '../app/workspace'
import { useOp } from '../api/ops'
import { Button, showToast } from '../ui'
import { useHostOffers } from './addPaths'

/**
 * A2.20: "Start one on a linked host": the offers of the hosts you may use
 * here (yours and the workspace's), in the hosts' own words, and this
 * server's own assistant (A1.25). Choosing one
 * tells the host who chose it and links to its page when it has one on its
 * own address; the host sets the agent up there (anything secret stays on the
 * host) and then proposes it here, which asks you to approve it. Shown only
 * when a host offers something.
 */

export function HostOfferList() {
  const workspaceId = useWorkspaceId() ?? undefined
  const listed = useHostOffers()
  const [page, setPage] = useState<{ hostName: string; url: string } | null>(null)
  const start = useOp('agent_offer_start', { workspaceId })
  if (listed.length === 0) return null
  return (
    <>
      <ul className="aw-agents__joinable">
        {listed.map(({ hostId, hostName, offer }) => {
          const id = String(offer.id ?? '')
          const title = typeof offer.title === 'string' ? offer.title : id
          const summary = typeof offer.summary === 'string' ? offer.summary : null
          // The built-in assistant (A1.25): says what to set when the
          // workspace has no model yet, and is added at once when chosen.
          const unavailable = typeof offer.unavailable === 'string' ? offer.unavailable : null
          const opening = offer.start as { adds?: boolean } | undefined
          const adds = opening?.adds === true
          return (
            <li className="aw-agents__joinable-row" key={`${hostId}:${id}`}>
              <span className="aw-agents__joinable-name">
                <strong>{title}</strong>{' '}
                <span className="aw-agents__joinable-handle">
                  {summary ? `${summary} · ` : ''}
                  {hostName}
                </span>
                {unavailable && <span className="aw-quiet"> {unavailable}</span>}
              </span>
              <Button
                busy={start.isPending && start.variables?.host === hostId && start.variables?.offerId === id}
                disabled={start.isPending || unavailable !== null}
                onClick={() =>
                  start.mutate(
                    { host: hostId, offerId: id },
                    {
                      onError: (error) =>
                        toastError(adds ? `${title} wasn’t added` : `${hostName} wasn’t reached`, error),
                      onSuccess: (result) => {
                        setPage(result.page ? { hostName, url: result.page } : null)
                        showToast(
                          result.accountId
                            ? {
                                title: `Added ${title}`,
                                description: 'It answers its DMs and mentions now.',
                              }
                            : {
                                title: `Sent to ${hostName}`,
                                description: 'When it’s done, it asks you here to approve the new agent.',
                              },
                        )
                      },
                    },
                  )
                }
              >
                {adds ? 'Add' : 'Start'}
              </Button>
            </li>
          )
        })}
      </ul>
      {page && (
        <p className="aw-quiet">
          Carry on at{' '}
          <a href={page.url} rel="noopener noreferrer" target="_blank">
            {page.hostName}’s page
          </a>
          ; anything secret, like a model key, is entered there and never passes through here.
        </p>
      )}
    </>
  )
}

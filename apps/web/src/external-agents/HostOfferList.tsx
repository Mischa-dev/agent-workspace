import { useState } from 'react'

import { toastError } from '../app/mutations'
import { useWorkspaceId } from '../app/workspace'
import { useOp } from '../api/ops'
import { Button, showToast } from '../ui'
import { useHostOffers } from './addPaths'

/**
 * A2.20: "Start one on a linked host": the offers of the hosts you may use
 * here (yours and the workspace's), in the hosts' own words. Choosing one
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
          return (
            <li className="aw-agents__joinable-row" key={`${hostId}:${id}`}>
              <span className="aw-agents__joinable-name">
                <strong>{title}</strong>{' '}
                <span className="aw-agents__joinable-handle">
                  {summary ? `${summary} · ` : ''}
                  {hostName}
                </span>
              </span>
              <Button
                busy={start.isPending && start.variables?.host === hostId && start.variables?.offerId === id}
                disabled={start.isPending}
                onClick={() =>
                  start.mutate(
                    { host: hostId, offerId: id },
                    {
                      onError: (error) => toastError(`${hostName} wasn’t reached`, error),
                      onSuccess: (result) => {
                        setPage(result.page ? { hostName, url: result.page } : null)
                        showToast({
                          title: `Sent to ${hostName}`,
                          description: 'When it’s done, it asks you here to approve the new agent.',
                        })
                      },
                    },
                  )
                }
              >
                Start
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

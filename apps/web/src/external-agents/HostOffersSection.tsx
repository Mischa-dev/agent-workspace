import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { toastError } from '../app/mutations'
import { useWorkspaceId } from '../app/workspace'
import { opQuery, useOp } from '../api/ops'
import { Button, Section, showToast } from '../ui'

/**
 * A2.20: "Start one on a linked host": the offers of the hosts you may use
 * here (yours and the workspace's), in the hosts' own words. Choosing one
 * tells the host who chose it and links to its page when it has one on its
 * own address; the host sets the agent up there (anything secret stays on the
 * host) and then proposes it here, which asks you to approve it. Shown only
 * when a host offers something.
 */
export function HostOffersSection() {
  const workspaceId = useWorkspaceId() ?? undefined
  const offers = useQuery(opQuery('agent_offer_list', {}, { workspaceId }))
  const [page, setPage] = useState<{ hostName: string; url: string } | null>(null)
  const start = useOp('agent_offer_start', { workspaceId })
  const listed = offers.data?.offers ?? []
  if (listed.length === 0) return null
  return (
    <Section
      description="Ways to get a new agent from the hosts you’ve linked. Each host walks you through its own steps."
      title="Start one on a linked host"
    >
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
    </Section>
  )
}

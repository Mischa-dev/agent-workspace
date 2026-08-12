import { useEffect, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import {
  listSponsoredExecutionRequests,
  respondToSponsoredExecution,
  type SponsoredExecutionRequest,
} from './sponsoredExecutionApi'

type DecisionInput = {
  request: SponsoredExecutionRequest
  decision: 'approve' | 'deny'
}

export function SponsoredExecutionSettings({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient()
  const requests = useQuery({
    queryKey: queryKeys.sponsoredExecutions(projectId),
    queryFn: () => listSponsoredExecutionRequests(projectId),
  })
  const [error, setError] = useState<string | null>(null)
  const errorRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])
  const response = useMutation({
    mutationFn: ({ request, decision }: DecisionInput) =>
      respondToSponsoredExecution(
        request.approvalId,
        request.id,
        request.version,
        decision,
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: queryKeys.sponsoredExecutions(projectId),
      })
    },
    onError: (reason) => {
      setError(
        isApiError(reason)
          ? reason.message
          : 'The sponsored execution request could not be resolved.',
      )
    },
  })

  if (requests.isPending) {
    return <p aria-live="polite">Loading sponsored requests…</p>
  }
  if (requests.isError) {
    return (
      <div className="error-state" role="alert">
        <p>Sponsored execution requests could not be loaded.</p>
        <button onClick={() => void requests.refetch()} type="button">
          Try again
        </button>
      </div>
    )
  }
  const pending = requests.data.requests.filter(
    (request) => request.status === 'pending',
  )
  return (
    <div className="external-agent-settings">
      {error && (
        <div
          className="form-error-summary"
          ref={errorRef}
          role="alert"
          tabIndex={-1}
        >
          {error}
        </div>
      )}
      {!pending.length && <p>No external Worker requests need your decision.</p>}
      <div className="external-agent-list">
        {pending.map((request) => (
          <article className="external-agent-row" key={request.id}>
            <div>
              <strong>{request.requesterDisplayName}</strong>
              <span>@{request.requesterHandle}</span>
              <p>{request.prompt}</p>
              <small>
                Human sponsorship required · expires {request.expiresAt}
              </small>
            </div>
            <div className="row-actions">
              <button
                className="primary-button"
                disabled={response.isPending}
                onClick={() => {
                  setError(null)
                  response.mutate({ request, decision: 'approve' })
                }}
                type="button"
              >
                {response.isPending ? 'Responding…' : 'Approve and queue'}
              </button>
              <button
                disabled={response.isPending}
                onClick={() => {
                  setError(null)
                  response.mutate({ request, decision: 'deny' })
                }}
                type="button"
              >
                Deny
              </button>
            </div>
          </article>
        ))}
      </div>
    </div>
  )
}

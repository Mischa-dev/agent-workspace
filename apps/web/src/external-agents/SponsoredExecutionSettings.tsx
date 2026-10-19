import { useEffect, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { ActorTag, Avatar, Banner, Button, EmptyState, ErrorState, ListSkeleton, Row, RowList, describeError, formatFullDate, formatRelative } from '../ui'
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
      respondToSponsoredExecution(request.approvalId, request.id, request.version, decision),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.sponsoredExecutions(projectId) })
    },
    onError: (reason, input) => {
      setError(`${input.decision === 'approve' ? 'It wasn’t approved' : 'It wasn’t denied'}. ${describeError(reason).message}`)
    },
  })

  if (requests.isPending) return <ListSkeleton label="Loading run requests" rows={2} />
  if (requests.isError) {
    return <ErrorState error={requests.error} onRetry={() => void requests.refetch()} title="Run requests didn’t load" />
  }
  const pending = requests.data.requests.filter((request) => request.status === 'pending')
  return (
    <div className="aw-settings-stack">
      {error && (
        <Banner ref={errorRef} tabIndex={-1} tone="danger">
          {error}
        </Banner>
      )}
      {!pending.length ? (
        <EmptyState icon="circle-check" title="No run requests are waiting for you">
          When an agent asks to run on your account, its request shows here.
        </EmptyState>
      ) : (
        <RowList label="Run requests">
          {pending.map((request) => (
            <Row
              actions={
                <>
                  <Button
                    busy={response.isPending && response.variables?.request.id === request.id && response.variables.decision === 'approve'}
                    disabled={response.isPending}
                    icon="play"
                    onClick={() => {
                      setError(null)
                      response.mutate({ request, decision: 'approve' })
                    }}
                    size="sm"
                    variant="primary"
                  >
                    Approve and queue
                  </Button>
                  <Button
                    disabled={response.isPending}
                    onClick={() => {
                      setError(null)
                      response.mutate({ request, decision: 'deny' })
                    }}
                    size="sm"
                    variant="ghost"
                  >
                    Deny
                  </Button>
                </>
              }
              key={request.id}
              leading={<Avatar kind="agent" name={request.requesterDisplayName} seed={request.requestingActorId} size={32} />}
              meta={
                <span title={`Expires ${formatFullDate(request.expiresAt)}`}>
                  @{request.requesterHandle} · asks to run on your account · expires {formatRelative(request.expiresAt)}
                </span>
              }
              title={
                <span className="aw-row__titleline">
                  {request.requesterDisplayName}
                  <ActorTag kind="agent" />
                </span>
              }
            >
              <blockquote className="aw-request__prompt">{request.prompt}</blockquote>
            </Row>
          ))}
        </RowList>
      )}
    </div>
  )
}

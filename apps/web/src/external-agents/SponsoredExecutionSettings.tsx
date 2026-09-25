import { useEffect, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { ActorTag, Avatar, Banner, Button, ErrorState, MessageSkeleton, Row, RowList } from '../ui'
import {
  listSponsoredExecutionRequests,
  respondToSponsoredExecution,
  type SponsoredExecutionRequest,
} from './sponsoredExecutionApi'

type DecisionInput = {
  request: SponsoredExecutionRequest
  decision: 'approve' | 'deny'
}

function formatWhen(iso: string) {
  const date = new Date(iso)
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString([], { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })
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
    onError: (reason) => {
      setError(isApiError(reason) ? reason.message : 'Your answer to that request didn’t go through. Try again.')
    },
  })

  if (requests.isPending) return <MessageSkeleton label="Loading run requests" rows={1} />
  if (requests.isError) {
    return (
      <ErrorState action={<Button onClick={() => void requests.refetch()}>Try again</Button>} title="Run requests didn’t load">
        Check your connection and try again.
      </ErrorState>
    )
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
        <p className="aw-quiet">No run requests are waiting for you.</p>
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
              meta={`@${request.requesterHandle} · asks to run Codex on your account · expires ${formatWhen(request.expiresAt)}`}
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

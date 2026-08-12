import { requestJson } from '../api/client'

export type SponsoredExecutionRequest = {
  id: string
  approvalId: string
  requestingActorId: string
  requesterDisplayName: string
  requesterHandle: string
  conversationId: string
  repositoryId: string
  prompt: string
  status: 'pending' | 'launched' | 'denied' | 'cancelled' | 'expired'
  version: number
  agentRunId: string | null
  expiresAt: string
  createdAt: string
}

export function listSponsoredExecutionRequests(projectId: string) {
  return requestJson<{ requests: SponsoredExecutionRequest[] }>(
    `/api/v1/projects/${encodeURIComponent(projectId)}/external-execution-requests`,
  )
}

export function respondToSponsoredExecution(
  approvalId: string,
  requestId: string,
  version: number,
  decision: 'approve' | 'deny',
) {
  return requestJson<unknown>(
    `/api/v1/agent-approvals/${encodeURIComponent(approvalId)}/respond`,
    {
      method: 'POST',
      json: { decision },
      idempotencyKey: `sponsored-${requestId}-${version}-${decision}`,
    },
  )
}

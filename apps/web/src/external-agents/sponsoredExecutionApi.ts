import { requestJson } from '../api/client'
import { callOp } from '../api/ops'

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

/** The sponsor answers an agent's run request: its approval is a request to them. */
export function respondToSponsoredExecution(approvalId: string, decision: 'approve' | 'deny') {
  return callOp('request_respond', { requestId: approvalId, optionId: decision })
}

import type { RequestView } from '../api/generated/operations'
import type {
  ConversationCapabilities,
  ConversationTimelineEntry,
  ConversationTimelinePage,
  ConversationWorkItemChip,
} from '../conversations/types'

export type RunState =
  | 'created'
  | 'queued'
  | 'starting'
  | 'working'
  | 'blocked'
  | 'recovering'
  | 'completed'
  | 'failed'
  | 'canceled'
  | 'interrupted'

export type Run = {
  id: string
  /** The settings frozen on the run when it started (harness, model, reasoning, speed). */
  config?: unknown
  state: RunState
  repositoryId: string
  repositoryWorkspaceId: string | null
  providerSessionId: string | null
  providerTurnId: string | null
  branch: string | null
  baseCommit: string | null
  publishedCommit: string | null
  publishedAt: string | null
  publishFailureCategory: string | null
  pullRequestRequestState: string | null
  pullRequestFailureCategory: string | null
  pullRequestNumber: number | null
  pullRequestId: string | null
  pullRequestUrl: string | null
  pullRequestTitle: string | null
  pullRequestState: string | null
  pullRequestDraft: boolean | null
  pullRequestChecksState: string | null
  pullRequestReviewState: string | null
  pullRequestMergeabilityState: string | null
  pullRequestSyncState: string | null
  pullRequestSyncFailureCategory: string | null
  pullRequestAutomationState: string | null
  pullRequestAutomationReason: string | null
  queuePosition: number | null
  failureReason: string | null
  failureAcknowledged: boolean
  createdAt: string
  updatedAt: string
  capabilities: {
    interrupt: boolean
    retry: boolean
    publish: boolean
    createPullRequest: boolean
    refreshPullRequest: boolean
  }
}
/** A request a run in this thread raised, and whether you may answer it. */
export type ThreadRequest = RequestView & { mayAnswer: boolean }

/** An answer to a request: an option, words, or both. */
export type RequestAnswer = { optionId?: string; text?: string }

export type Timeline = {
  conversation: ConversationTimelinePage['conversation']
  linkedWorkItems: ConversationWorkItemChip[]
  entries: ConversationTimelineEntry[]
  nextCursor: string | null
  highestSequence: number
  readState: {
    highestSeenSequence: number
    version: number
  }
  activeRun: Run | null
  latestTerminalRun: Run | null
  requests: ThreadRequest[]
  capabilities: ConversationCapabilities & { workerInvoke: boolean }
  replyRequests: ConversationTimelinePage['replyRequests']
}

import { useQuery } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { queryKeys } from '../api/queryKeys'
import { normalizeTimelinePage } from '../conversations/api'
import type { Approval, Run, Timeline } from './threadTypes'

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object'
    ? (value as Record<string, unknown>)
    : {}
}

function explicitCapability(value: unknown, name: string) {
  return asRecord(value)[name] === true
}

export function normalizeTimeline(value: unknown): Timeline {
  const record = asRecord(value)
  const durable = normalizeTimelinePage(value)
  const activeRunRecord = asRecord(record.activeRun)
  const activeRun =
    typeof activeRunRecord.id === 'string'
      ? ({
          ...activeRunRecord,
          providerTurnId:
            typeof activeRunRecord.providerTurnId === 'string'
              ? activeRunRecord.providerTurnId
              : null,
          branch:
            typeof activeRunRecord.branch === 'string'
              ? activeRunRecord.branch
              : null,
          baseCommit:
            typeof activeRunRecord.baseCommit === 'string'
              ? activeRunRecord.baseCommit
              : null,
          publishedCommit:
            typeof activeRunRecord.publishedCommit === 'string'
              ? activeRunRecord.publishedCommit
              : null,
          publishedAt:
            typeof activeRunRecord.publishedAt === 'string'
              ? activeRunRecord.publishedAt
              : null,
          publishFailureCategory:
            typeof activeRunRecord.publishFailureCategory === 'string'
              ? activeRunRecord.publishFailureCategory
              : null,
          pullRequestRequestState: null,
          pullRequestFailureCategory: null,
          pullRequestNumber: null,
          pullRequestId: null,
          pullRequestUrl: null,
          pullRequestTitle: null,
          pullRequestState: null,
          pullRequestDraft: null,
          pullRequestChecksState: null,
          pullRequestReviewState: null,
          pullRequestMergeabilityState: null,
          pullRequestSyncState: null,
          pullRequestSyncFailureCategory: null,
          pullRequestAutomationState: null,
          pullRequestAutomationReason: null,
          queuePosition:
            typeof activeRunRecord.queuePosition === 'number'
              ? activeRunRecord.queuePosition
              : null,
          failureAcknowledged: false,
          capabilities: {
            interrupt: explicitCapability(
              activeRunRecord.capabilities,
              'interrupt',
            ),
            retry: explicitCapability(activeRunRecord.capabilities, 'retry'),
            publish: false,
            createPullRequest: false,
            refreshPullRequest: false,
          },
        } as Run)
      : null
  const latestTerminalRunRecord = asRecord(record.latestTerminalRun)
  const latestTerminalRun =
    typeof latestTerminalRunRecord.id === 'string'
      ? ({
          ...latestTerminalRunRecord,
          providerTurnId:
            typeof latestTerminalRunRecord.providerTurnId === 'string'
              ? latestTerminalRunRecord.providerTurnId
              : null,
          branch:
            typeof latestTerminalRunRecord.branch === 'string'
              ? latestTerminalRunRecord.branch
              : null,
          baseCommit:
            typeof latestTerminalRunRecord.baseCommit === 'string'
              ? latestTerminalRunRecord.baseCommit
              : null,
          publishedCommit:
            typeof latestTerminalRunRecord.publishedCommit === 'string'
              ? latestTerminalRunRecord.publishedCommit
              : null,
          publishedAt:
            typeof latestTerminalRunRecord.publishedAt === 'string'
              ? latestTerminalRunRecord.publishedAt
              : null,
          publishFailureCategory:
            typeof latestTerminalRunRecord.publishFailureCategory === 'string'
              ? latestTerminalRunRecord.publishFailureCategory
              : null,
          pullRequestRequestState:
            typeof latestTerminalRunRecord.pullRequestRequestState === 'string'
              ? latestTerminalRunRecord.pullRequestRequestState
              : null,
          pullRequestFailureCategory:
            typeof latestTerminalRunRecord.pullRequestFailureCategory ===
            'string'
              ? latestTerminalRunRecord.pullRequestFailureCategory
              : null,
          pullRequestNumber:
            typeof latestTerminalRunRecord.pullRequestNumber === 'number'
              ? latestTerminalRunRecord.pullRequestNumber
              : null,
          pullRequestId:
            typeof latestTerminalRunRecord.pullRequestId === 'string'
              ? latestTerminalRunRecord.pullRequestId
              : null,
          pullRequestUrl:
            typeof latestTerminalRunRecord.pullRequestUrl === 'string'
              ? latestTerminalRunRecord.pullRequestUrl
              : null,
          pullRequestTitle:
            typeof latestTerminalRunRecord.pullRequestTitle === 'string'
              ? latestTerminalRunRecord.pullRequestTitle
              : null,
          pullRequestState:
            typeof latestTerminalRunRecord.pullRequestState === 'string'
              ? latestTerminalRunRecord.pullRequestState
              : null,
          pullRequestDraft:
            typeof latestTerminalRunRecord.pullRequestDraft === 'boolean'
              ? latestTerminalRunRecord.pullRequestDraft
              : null,
          pullRequestChecksState:
            typeof latestTerminalRunRecord.pullRequestChecksState === 'string'
              ? latestTerminalRunRecord.pullRequestChecksState
              : null,
          pullRequestReviewState:
            typeof latestTerminalRunRecord.pullRequestReviewState === 'string'
              ? latestTerminalRunRecord.pullRequestReviewState
              : null,
          pullRequestMergeabilityState:
            typeof latestTerminalRunRecord.pullRequestMergeabilityState ===
            'string'
              ? latestTerminalRunRecord.pullRequestMergeabilityState
              : null,
          pullRequestSyncState:
            typeof latestTerminalRunRecord.pullRequestSyncState === 'string'
              ? latestTerminalRunRecord.pullRequestSyncState
              : null,
          pullRequestSyncFailureCategory:
            typeof latestTerminalRunRecord.pullRequestSyncFailureCategory ===
            'string'
              ? latestTerminalRunRecord.pullRequestSyncFailureCategory
              : null,
          pullRequestAutomationState:
            typeof latestTerminalRunRecord.pullRequestAutomationState ===
            'string'
              ? latestTerminalRunRecord.pullRequestAutomationState
              : null,
          pullRequestAutomationReason:
            typeof latestTerminalRunRecord.pullRequestAutomationReason ===
            'string'
              ? latestTerminalRunRecord.pullRequestAutomationReason
              : null,
          queuePosition: null,
          failureAcknowledged:
            latestTerminalRunRecord.failureAcknowledged === true,
          capabilities: {
            interrupt: false,
            retry: explicitCapability(
              latestTerminalRunRecord.capabilities,
              'retry',
            ),
            publish: explicitCapability(
              latestTerminalRunRecord.capabilities,
              'publish',
            ),
            createPullRequest: explicitCapability(
              latestTerminalRunRecord.capabilities,
              'createPullRequest',
            ),
            refreshPullRequest: explicitCapability(
              latestTerminalRunRecord.capabilities,
              'refreshPullRequest',
            ),
          },
        } as Run)
      : null
  const approvals = Array.isArray(record.approvals)
    ? record.approvals
        .map((value) => asRecord(value))
        .filter((approval) => typeof approval.id === 'string')
        .map(
          (approval) =>
            ({
              ...approval,
              capabilities: {
                respond: explicitCapability(
                  approval.capabilities,
                  'respond',
                ),
              },
            }) as Approval,
        )
    : []
  return {
    ...durable,
    activeRun,
    latestTerminalRun,
    approvals,
    capabilities: {
      ...durable.capabilities,
      workerInvoke: explicitCapability(
        record.capabilities,
        'workerInvoke',
      ),
    },
  }
}

export function useThread(
  projectId: string,
  threadId: string,
  includeArchived: boolean,
) {
  const timeline = useQuery({
    queryKey: queryKeys.timeline(projectId, threadId, includeArchived),
    queryFn: async () =>
      normalizeTimeline(
        await requestJson<unknown>(
          `/api/v1/conversations/${encodeURIComponent(threadId)}/timeline${
            includeArchived ? '?includeArchived=true' : ''
          }`,
        ),
      ),
    enabled: Boolean(threadId),
    retry: false,
  })
  const routeMatches = Boolean(
    timeline.data?.conversation.projectId === projectId &&
      timeline.data.conversation.id === threadId &&
      timeline.data.conversation.kind === 'thread',
  )

  return {
    routeMatches,
    timeline,
  }
}

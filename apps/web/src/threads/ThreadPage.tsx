import {
  FormEvent,
  useCallback,
  useMemo,
  useState,
} from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Link,
  Navigate,
  useNavigate,
  useParams,
  useSearchParams,
} from 'react-router-dom'

import { requestJson } from '../api/client'
import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { useAuthenticatedSession } from '../auth/session'
import { useProject } from '../projects/hooks'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import {
  ProjectPageError,
  ProjectPageLoading,
} from '../projects/ProjectPageStates'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import {
  isSafeMessageText,
  messageTextScalarValues,
} from '../conversations/messageContentModel'
import { StructuredComposer } from '../conversations/StructuredComposer'
import type { ComposerSubmission } from '../conversations/composerModel'
import { ConversationHeader } from '../conversations/ConversationHeader'
import { ThreadStatusButton } from '../conversations/threadBoard'
import { useProjectConversationSidebar } from '../conversations/hooks'
import { useMessageActions } from '../conversations/MessageActions'
import { ReplyStatusList } from '../conversations/ReplyStatus'
import { groupReplyRequests } from '../conversations/replyStatusModel'
import { ConversationTimeline, OlderMessagesButton, PendingMessageRow, TimelineEntries } from '../conversations/Timeline'
import { useAutoMarkSeen } from '../conversations/useAutoMarkSeen'
import { useArchiveConversation } from '../conversations/useArchiveConversation'
import { useLiveOlderEntries } from '../conversations/useLiveOlderEntries'
import { useInstantMutation } from '../app/mutations'
import { usePageTitle } from '../shell/documentTitleModel'
import {
  normalizeTimelinePage,
  retryReplyRequest,
  searchMentionableActors,
  sendConversationMessage,
  stopReplyRequest,
} from '../conversations/api'
import type {
  ConversationCapabilities,
  ConversationTimelineEntry,
  ConversationTimelinePage,
  ConversationWorkItemReference,
} from '../conversations/types'
import { useConversationRealtime } from '../realtime/realtimeContext'
import { useSetWorkItemThreadLink } from '../work-items/hooks'
import { searchWorkItemReferences } from '../work-items/api'
import {
  ActorTag,
  Avatar,
  Banner,
  Button,
  ConversationIntro,
  Dialog,
  Dots,
  ErrorState,
  Icon,
  IconButton,
  MessageSkeleton,
  SegmentedControl,
} from '../ui'
import { ApprovalCard, RunBadge, RunCard, ThreadDetails } from './ThreadPresentation'

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
export type Approval = {
  id: string
  agentRunId: string
  kind: 'approval' | 'question'
  summary: string
  options: Array<{ id: string; label: string }>
  status: string
  response: { decision?: string; answer?: string } | null
  createdAt: string
  capabilities: {
    respond: boolean
  }
}

type Timeline = {
  conversation: ConversationTimelinePage['conversation']
  linkedWorkItems: ConversationWorkItemReference[]
  entries: ConversationTimelineEntry[]
  nextCursor: string | null
  highestSequence: number
  readState: {
    highestSeenSequence: number
    version: number
  }
  activeRun: Run | null
  latestTerminalRun: Run | null
  approvals: Approval[]
  capabilities: ConversationCapabilities & { workerInvoke: boolean }
  replyRequests: ConversationTimelinePage['replyRequests']
}

const NO_ENTRIES: ConversationTimelineEntry[] = []

// What a run action failed to do, for its error toast.
const ACTION_ERRORS: Record<string, string> = {
  start: 'Couldn’t start the run',
  interrupt: 'Couldn’t stop the run',
  retry: 'Couldn’t run it again',
  publish: 'Couldn’t publish the branch',
  'pull-request': 'Couldn’t open the pull request',
  'pull-request-refresh': 'Couldn’t check GitHub',
  'acknowledge-failure': 'Couldn’t dismiss the failure',
}

const post = (path: string, idempotencyKey: string, json: unknown = {}) =>
  requestJson(path, { method: 'POST', idempotencyKey, json })

/** The thread's run commands, one request each. */
const threadApi = {
  startRun: (threadId: string, prompt: string, key: string) =>
    post(`/api/v1/threads/${encodeURIComponent(threadId)}/agent-runs`, key, { prompt }),
  respond: (approvalId: string, action: string, response: { decision: 'approve' | 'deny' } | { answer: string }) =>
    post(`/api/v1/agent-approvals/${encodeURIComponent(approvalId)}/respond`, `approval-${approvalId}-${action}`, response),
  interrupt: (runId: string) => post(`/api/v1/agent-runs/${encodeURIComponent(runId)}/interrupt`, `interrupt-${runId}`),
  retry: (runId: string) => post(`/api/v1/agent-runs/${encodeURIComponent(runId)}/retry`, `retry-${runId}`),
  publish: (projectId: string, runId: string, key: string) =>
    post(`/api/v1/projects/${encodeURIComponent(projectId)}/agent-runs/${encodeURIComponent(runId)}/publish`, key),
  createPullRequest: (projectId: string, runId: string, key: string, json: { title: string; body: string; workItemIds: string[] }) =>
    post(`/api/v1/projects/${encodeURIComponent(projectId)}/agent-runs/${encodeURIComponent(runId)}/pull-request`, key, json),
  refreshPullRequest: (projectId: string, pullRequestId: string) =>
    post(`/api/v1/projects/${encodeURIComponent(projectId)}/pull-requests/${encodeURIComponent(pullRequestId)}/refresh`, crypto.randomUUID()),
  acknowledgeFailure: (runId: string, eventId: string) =>
    post(`/api/v1/agent-runs/${encodeURIComponent(runId)}/acknowledge-failure`, `acknowledge-failure-${runId}-${eventId}`, { eventId }),
}

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object'
    ? (value as Record<string, unknown>)
    : {}
}

function explicitCapability(value: unknown, name: string) {
  return asRecord(value)[name] === true
}

function normalizeTimeline(value: unknown): Timeline {
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

function useThread(
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

export function ThreadPage() {
  const session = useAuthenticatedSession()
  const { projectId = '', threadId = '' } = useParams()
  const [searchParams] = useSearchParams()
  const includeArchived = searchParams.get('includeArchived') === 'true'
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const project = useProject(projectId)
  const thread = useThread(projectId, threadId, includeArchived)
  const workItemLink = useSetWorkItemThreadLink()
  const realtime = useConversationRealtime(threadId)
  const timeline = thread.timeline.data
  const verifiedTimeline = thread.routeMatches ? timeline : undefined
  usePageTitle(verifiedTimeline?.conversation.name ?? null)
  const activeRun = verifiedTimeline?.activeRun ?? null
  const latestTerminalRun = verifiedTimeline?.latestTerminalRun ?? null
  const [prompt, setPrompt] = useState('')
  const [startIdempotencyKey, setStartIdempotencyKey] = useState<string | null>(
    null,
  )
  const [publishIdempotencyKey, setPublishIdempotencyKey] = useState<
    string | null
  >(null)
  const [pullRequestIdempotencyKey, setPullRequestIdempotencyKey] = useState<
    string | null
  >(null)
  const [acknowledgedRunId, setAcknowledgedRunId] = useState<string | null>(null)
  const [detailsOpen, setDetailsOpen] = useState(false)
  const [pendingSends, setPendingSends] = useState<{ key: string; text?: string }[]>([])
  const [archiveOpen, setArchiveOpen] = useState(false)
  const [composerMode, setComposerMode] = useState<'message' | 'run'>('message')
  const parentSidebar = useProjectConversationSidebar(projectId, verifiedTimeline?.conversation.parentChannelId ?? '')
  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'thread', conversationId: threadId },
    project.isSuccess &&
      project.data.capabilities.navigationWrite &&
      thread.timeline.isSuccess &&
      thread.routeMatches &&
      !includeArchived &&
      !verifiedTimeline?.conversation.archivedAt,
    project.data?.navigation,
  )

  const pendingApproval = useMemo(
    () =>
      verifiedTimeline?.approvals.find(
        (approval) => approval.status === 'pending',
      ),
    [verifiedTimeline],
  )
  const older = useLiveOlderEntries({
    conversationId: threadId,
    newestEntries: verifiedTimeline?.entries ?? NO_ENTRIES,
    newestCursor: verifiedTimeline?.nextCursor ?? null,
    loadPage: async (cursor) => {
      const parameters = new URLSearchParams({ limit: '50', cursor })
      if (includeArchived) parameters.set('includeArchived', 'true')
      const result = normalizeTimeline(
        await requestJson<unknown>(
          `/api/v1/conversations/${encodeURIComponent(threadId)}/timeline?${parameters}`,
        ),
      )
      if (
        result.conversation.id !== threadId ||
        result.conversation.projectId !== projectId ||
        result.conversation.kind !== 'thread'
      ) {
        throw new Error('Older timeline page crossed its Project boundary.')
      }
      return result
    },
  })
  const entries = older.entries
  // Agents at work here. A channel tag answered in this thread is about the
  // channel message it started from, so it shows under the thread's messages.
  const replyRequests = verifiedTimeline?.replyRequests
  const requestsByMessage = useMemo(() => groupReplyRequests(replyRequests ?? []), [replyRequests])
  const trailingRequests = useMemo(() => {
    const shown = new Set(entries.flatMap((entry) => (entry.message ? [entry.message.id] : [])))
    return (replyRequests ?? []).filter((request) => !shown.has(request.messageId))
  }, [entries, replyRequests])
  const retryAgent = (requestId: string) => void retryReplyRequest(requestId).then(() => thread.timeline.refetch())
  const stopAgent = verifiedTimeline?.capabilities.sendMessage
    ? (requestId: string) => stopReplyRequest(requestId).then(() => thread.timeline.refetch())
    : undefined
  const latestFailureEvent = useMemo(
    () =>
      latestTerminalRun?.state === 'failed'
        ? [...entries]
            .reverse()
            .find(
              (entry) =>
                entry.workerEvent?.agentRunId === latestTerminalRun.id &&
                entry.workerEvent.kind === 'run_failed',
            )?.workerEvent ?? null
        : null,
    [entries, latestTerminalRun],
  )
  const olderCursor = older.cursor

  const onSeen = useCallback(
    () =>
      queryClient.invalidateQueries({
        queryKey: queryKeys.conversationSidebarRoot(projectId),
      }),
    [projectId, queryClient],
  )
  useAutoMarkSeen(
    {
      conversationId: threadId,
      highestSequence: verifiedTimeline?.highestSequence ?? 0,
      highestSeenSequence: verifiedTimeline?.readState.highestSeenSequence ?? 0,
      // Only once the newest entry is on screen.
      ready: Boolean(
        verifiedTimeline?.capabilities.markSeen &&
          entries.at(-1)?.sequence === verifiedTimeline.highestSequence,
      ),
    },
    onSeen,
  )

  const action = useInstantMutation({
    mutationFn: ({ request }: { key: string; request: () => Promise<unknown> }) => request(),
    errorTitle: ({ key }) => ACTION_ERRORS[key] ?? (key.includes(':') ? 'Couldn’t send your answer' : 'That didn’t work'),
    invalidate: () => [queryKeys.timeline(projectId, threadId, includeArchived)],
    onSuccess: (_result, { key }) => {
      if (key === 'start') setPrompt('')
    },
  })
  const busyAction = action.isPending ? action.variables.key : null
  const perform = (key: string, request: () => Promise<unknown>) =>
    action.mutateAsync({ key, request }).then(
      () => true,
      () => false,
    )
  const archiveThread = useArchiveConversation({
    conversationId: threadId,
    kind: 'thread',
    label: `“${verifiedTimeline?.conversation.name ?? 'thread'}”`,
    leaveTo: verifiedTimeline?.conversation.parentChannelId
      ? `/projects/${encodeURIComponent(projectId)}/channels/${encodeURIComponent(verifiedTimeline.conversation.parentChannelId)}`
      : `/projects/${encodeURIComponent(projectId)}`,
    projectId,
  })

  const startRun = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (verifiedTimeline?.capabilities.workerInvoke !== true || activeRun) {
      return
    }
    const value = prompt.trim()
    if (!value || !isSafeMessageText(value)) return
    const commandKey = startIdempotencyKey ?? crypto.randomUUID()
    if (!startIdempotencyKey) setStartIdempotencyKey(commandKey)
    void perform('start', () => threadApi.startRun(threadId, value, commandKey)).then((succeeded) => {
      if (succeeded) setStartIdempotencyKey(null)
    })
  }

  const respond = (
    approvalId: string,
    response: { decision: 'approve' | 'deny' } | { answer: string },
  ) => {
    const approval = verifiedTimeline?.approvals.find(
      (candidate) => candidate.id === approvalId,
    )
    if (
      !approval ||
      approval.status !== 'pending' ||
      approval.capabilities.respond !== true
    ) {
      return
    }
    const kind = 'decision' in response ? response.decision : 'answer'
    void perform(`${approvalId}:${kind}`, () => threadApi.respond(approvalId, kind, response))
  }

  const interrupt = () => {
    const runId = activeRun?.id
    if (!runId || activeRun.capabilities.interrupt !== true) return
    void perform('interrupt', () => threadApi.interrupt(runId))
  }

  const retryLastRun = () => {
    const runId = latestTerminalRun?.id
    if (!runId || latestTerminalRun.capabilities.retry !== true || activeRun) return
    void perform('retry', () => threadApi.retry(runId))
  }

  const publishLastRun = () => {
    const runId = latestTerminalRun?.id
    if (
      !runId ||
      !latestTerminalRun.repositoryWorkspaceId ||
      latestTerminalRun.capabilities.publish !== true ||
      activeRun
    ) {
      return
    }
    const commandKey = publishIdempotencyKey ?? crypto.randomUUID()
    if (!publishIdempotencyKey) setPublishIdempotencyKey(commandKey)
    void perform('publish', () => threadApi.publish(projectId, runId, commandKey)).then((succeeded) => {
      if (succeeded) setPublishIdempotencyKey(null)
    })
  }

  const createPullRequest = () => {
    const runId = latestTerminalRun?.id
    if (
      !runId ||
      latestTerminalRun.capabilities.createPullRequest !== true ||
      activeRun
    ) {
      return
    }
    const firstWorkItem = verifiedTimeline?.linkedWorkItems[0]
    const rawTitle = firstWorkItem
      ? `${firstWorkItem.key}: ${firstWorkItem.title}`
      : verifiedTimeline?.conversation.name || 'Agent Workspace change'
    const title = Array.from(rawTitle).slice(0, 256).join('')
    const commandKey = pullRequestIdempotencyKey ?? crypto.randomUUID()
    if (!pullRequestIdempotencyKey) setPullRequestIdempotencyKey(commandKey)
    void perform('pull-request', () =>
      threadApi.createPullRequest(projectId, runId, commandKey, {
        title,
        body: 'Created from an explicitly published Agent Workspace run.',
        workItemIds: verifiedTimeline?.linkedWorkItems.map((item) => item.id) ?? [],
      }),
    ).then((succeeded) => {
      if (succeeded) setPullRequestIdempotencyKey(null)
    })
  }

  const refreshPullRequest = () => {
    const pullRequestId = latestTerminalRun?.pullRequestId
    if (
      !pullRequestId ||
      latestTerminalRun.capabilities.refreshPullRequest !== true
    ) {
      return
    }
    void perform('pull-request-refresh', () => threadApi.refreshPullRequest(projectId, pullRequestId))
  }

  const acknowledgeFailure = () => {
    if (
      !latestTerminalRun ||
      !latestFailureEvent ||
      latestTerminalRun.failureAcknowledged ||
      acknowledgedRunId === latestTerminalRun.id
    ) {
      return
    }
    const runId = latestTerminalRun.id
    const eventId = latestFailureEvent.id
    void perform('acknowledge-failure', () => threadApi.acknowledgeFailure(runId, eventId)).then((succeeded) => {
      if (succeeded) setAcknowledgedRunId(runId)
    })
  }

  const searchActors = async (query: string) => (await searchMentionableActors(projectId, query)).actors
  const searchWorkItems = (query: string) => searchWorkItemReferences(projectId, query)
  const messageActions = useMessageActions({
    projectId,
    conversationId: threadId,
    viewerId: session.user.actorId,
    capabilities: verifiedTimeline?.capabilities,
    searchActors,
    searchWorkItems,
    onReactionsChanged: older.patchReactions,
    onChanged: (entry) => {
      older.replaceEntry(entry)
      void thread.timeline.refetch()
    },
  })

  const sendMessage = async (submission: ComposerSubmission) => {
    setPendingSends((current) => [...current, { key: submission.idempotencyKey, text: submission.text }])
    try {
      await sendConversationMessage({
        conversationId: threadId,
        kind: 'thread',
        ...submission,
      })
      await thread.timeline.refetch()
    } finally {
      setPendingSends((current) => current.filter((item) => item.key !== submission.idempotencyKey))
    }
  }

  if (isApiError(thread.timeline.error) && thread.timeline.error.status === 404) {
    const parent = verifiedTimeline?.conversation.parentChannelId ?? null
    return (
      <Navigate
        replace
        to={
          parent
            ? `/projects/${encodeURIComponent(projectId)}/channels/${encodeURIComponent(parent)}`
            : `/projects/${encodeURIComponent(projectId)}`
        }
      />
    )
  }

  if (thread.timeline.isSuccess && !thread.routeMatches) {
    return (
      <Navigate
        replace
        to={`/projects/${encodeURIComponent(projectId)}`}
      />
    )
  }

  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading />
      </AppShell>
    )
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <ProjectPageError onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  const threadName = verifiedTimeline?.conversation.name ?? 'Thread'
  const parentId = verifiedTimeline?.conversation.parentChannelId ?? null
  const parentChannel = parentSidebar.data?.channels.find((channel) => channel.id === parentId)
  const archived = Boolean(verifiedTimeline?.conversation.archivedAt)
  const canMessage = Boolean(verifiedTimeline?.capabilities.sendMessage && !archived)
  const showRunComposer = Boolean(
    verifiedTimeline && !archived && (activeRun ? activeRun.capabilities.interrupt : verifiedTimeline.capabilities.workerInvoke),
  )
  const mode: 'message' | 'run' = !canMessage ? 'run' : !showRunComposer ? 'message' : composerMode
  const runForCard = activeRun ?? latestTerminalRun
  const channelRoute = parentId
    ? `/projects/${encodeURIComponent(projectId)}/channels/${encodeURIComponent(parentId)}`
    : `/projects/${encodeURIComponent(projectId)}`

  const details =
    verifiedTimeline && detailsOpen ? (
      <ThreadDetails
        onClose={() => setDetailsOpen(false)}
        onUnlink={
          verifiedTimeline.capabilities.sendMessage
            ? (workItemId) => {
                workItemLink.mutate(
                  { projectId, workItemId, threadId, linked: false, idempotencyKey: crypto.randomUUID() },
                  { onSuccess: () => void thread.timeline.refetch() },
                )
              }
            : undefined
        }
        projectName={project.data.name}
        run={activeRun ?? latestTerminalRun}
        threadId={threadId}
        unlinking={workItemLink.isPending}
        workItems={verifiedTimeline.linkedWorkItems}
      />
    ) : undefined

  const busy = Boolean(busyAction)
  const runActions = runForCard ? (
    activeRun ? (
      activeRun.capabilities.interrupt ? (
        <Button busy={busyAction === 'interrupt'} icon="stop" onClick={interrupt} size="sm">
          {busyAction === 'interrupt' ? 'Stopping…' : 'Stop run'}
        </Button>
      ) : null
    ) : (
      <>
        {latestTerminalRun?.capabilities.createPullRequest ? (
          <Button busy={busyAction === 'pull-request'} disabled={busy} icon="pull-request" onClick={createPullRequest} size="sm" variant="primary">
            {busyAction === 'pull-request' ? 'Opening pull request…' : 'Create pull request'}
          </Button>
        ) : null}
        {latestTerminalRun?.capabilities.publish && latestTerminalRun.repositoryWorkspaceId ? (
          <Button busy={busyAction === 'publish'} disabled={busy} icon="branch" onClick={publishLastRun} size="sm">
            {busyAction === 'publish' ? 'Publishing…' : 'Publish branch'}
          </Button>
        ) : null}
        {latestTerminalRun?.capabilities.retry ? (
          <Button busy={busyAction === 'retry'} disabled={busy} icon="retry" onClick={retryLastRun} size="sm">
            {busyAction === 'retry' ? 'Starting again…' : 'Run again'}
          </Button>
        ) : null}
        {latestTerminalRun?.pullRequestId && latestTerminalRun.capabilities.refreshPullRequest ? (
          <Button busy={busyAction === 'pull-request-refresh'} disabled={busy} icon="retry" onClick={refreshPullRequest} size="sm" variant="ghost">
            {busyAction === 'pull-request-refresh' ? 'Checking GitHub…' : 'Refresh GitHub status'}
          </Button>
        ) : null}
        {latestFailureEvent && !latestTerminalRun?.failureAcknowledged && acknowledgedRunId !== latestTerminalRun?.id ? (
          <Button busy={busyAction === 'acknowledge-failure'} disabled={busy} onClick={acknowledgeFailure} size="sm" variant="ghost">
            {busyAction === 'acknowledge-failure' ? 'Dismissing…' : 'Dismiss failure'}
          </Button>
        ) : null}
      </>
    )
  ) : null

  const runNotes =
    latestTerminalRun && !activeRun ? (
      <>
        {latestTerminalRun.publishFailureCategory ? (
          <p className="aw-run__error" role="status">
            <Icon name="circle-alert" size={14} />
            Publishing needs attention: {latestTerminalRun.publishFailureCategory.replaceAll('_', ' ')}.
          </p>
        ) : null}
        {latestTerminalRun.pullRequestRequestState && !latestTerminalRun.pullRequestUrl ? (
          <p className="aw-run__detail">Pull request: {latestTerminalRun.pullRequestRequestState.replaceAll('_', ' ')}</p>
        ) : null}
        {latestTerminalRun.pullRequestFailureCategory ? (
          <p className="aw-run__error" role="status">
            <Icon name="circle-alert" size={14} />
            The pull request needs attention: {latestTerminalRun.pullRequestFailureCategory.replaceAll('_', ' ')}.
          </p>
        ) : null}
        {latestTerminalRun.pullRequestSyncState === 'failed' ? (
          <p className="aw-run__error" role="status">
            <Icon name="circle-alert" size={14} />
            GitHub status needs attention: {(latestTerminalRun.pullRequestSyncFailureCategory ?? 'sync_failed').replaceAll('_', ' ')}.
          </p>
        ) : null}
        {latestTerminalRun.pullRequestAutomationState && latestTerminalRun.pullRequestAutomationState !== 'not_evaluated' ? (
          <p className="aw-run__detail">
            Work item automation: {latestTerminalRun.pullRequestAutomationState.replaceAll('_', ' ')}
            {latestTerminalRun.pullRequestAutomationReason ? ` · ${latestTerminalRun.pullRequestAutomationReason.replaceAll('_', ' ')}` : ''}
          </p>
        ) : null}
      </>
    ) : null

  const modeSwitch =
    canMessage && showRunComposer ? (
      <SegmentedControl
        label="What to send"
        onChange={setComposerMode}
        options={[
          { value: 'message', label: 'Message', icon: 'message' },
          { value: 'run', label: 'Run with Codex', icon: 'terminal' },
        ]}
        value={mode}
      />
    ) : null

  return (
    <AppShell
      context={details}
      contextKey={details ? 'details' : undefined}
      mainClassName="aw-main--conversation"
      onContextDismiss={() => setDetailsOpen(false)}
      project={project.data}
      selectedChannelId={parentId ?? undefined}
      thread={verifiedTimeline ? { id: threadId, name: threadName } : null}
      user={session.user}
    >
      <NavigationSyncAlert {...navigationSync} />
      <ConversationHeader
        actions={
          verifiedTimeline ? (
            <>
              <ThreadStatusButton
                onSaved={() => void thread.timeline.refetch()}
                thread={{
                  id: threadId,
                  name: threadName,
                  status: verifiedTimeline.conversation.threadStatus ?? 'Open',
                  outcome: verifiedTimeline.conversation.threadOutcome,
                }}
              />
              <IconButton
                icon="panel-right"
                label={detailsOpen ? 'Hide details' : 'Show details'}
                onClick={() => setDetailsOpen(!detailsOpen)}
                pressed={detailsOpen}
                side="bottom"
              />
            </>
          ) : null
        }
        back={{ label: parentChannel ? `Back to #${parentChannel.name}` : 'Back to the channel', to: channelRoute }}
        icon="messages"
        menu={
          verifiedTimeline?.capabilities.archive && !archived
            ? [
                { label: parentChannel ? `Back to #${parentChannel.name}` : 'Back to the channel', icon: 'arrow-left', onSelect: () => navigate(channelRoute) },
                { separator: true },
                { label: 'Archive thread', icon: 'archive', danger: true, onSelect: () => setArchiveOpen(true) },
              ]
            : undefined
        }
        status={<RunBadge run={activeRun} />}
        subtitle={
          parentChannel ? (
            <Link className="aw-convhead__crumb" to={channelRoute}>
              in #{parentChannel.name}
            </Link>
          ) : undefined
        }
        title={threadName}
      />
      {archived ? (
        <Banner className="aw-conv__banner" icon="archive" title="This thread is archived." tone="offline">
          You can read it, but no one can post or run work here.
        </Banner>
      ) : null}

      <ConversationTimeline
        busy={thread.timeline.isPending}
        label={`Messages in ${threadName}`}
        revision={`${verifiedTimeline?.highestSequence ?? 0}:${pendingSends.length}:${Object.values(realtime.streaming).join('').length}:${pendingApproval?.id ?? ''}:${runForCard?.state ?? ''}`}
      >
        {olderCursor ? <OlderMessagesButton busy={older.loading} onClick={() => void older.loadOlder()} /> : null}
        {older.error ? <Banner tone="danger">{older.error}</Banner> : null}
        {thread.timeline.isPending ? <MessageSkeleton label="Loading thread" /> : null}
        {thread.timeline.isError ? (
          <ErrorState action={<Button onClick={() => void thread.timeline.refetch()}>Try again</Button>} title="This thread didn’t load">
            It may have been removed from this Project, or it couldn’t be reached.
          </ErrorState>
        ) : null}
        {verifiedTimeline && !olderCursor ? (
          <ConversationIntro icon="messages" title={threadName}>
            {verifiedTimeline.capabilities.workerInvoke
              ? 'A focused conversation. Ask Codex to make a change here and it runs in its own workspace, with every step, approval and result kept in this thread.'
              : 'A focused conversation inside the channel. Everything said here stays with this thread.'}
          </ConversationIntro>
        ) : null}
        <TimelineEntries
          entries={entries}
          onOpenHistory={messageActions.openHistory}
          renderEditor={messageActions.editorFor}
          renderExtras={(entry) =>
            entry.message && !entry.message.removed ? (
              <ReplyStatusList onRetry={retryAgent} onStop={stopAgent} requests={requestsByMessage.get(entry.message.id) ?? []} />
            ) : null
          }
          renderReactions={messageActions.reactionsFor}
          renderToolbar={messageActions.toolbarFor}
          viewerId={session.user.actorId}
        />
        {trailingRequests.length ? (
          <div className="aw-replies--trailing">
            <ReplyStatusList onRetry={retryAgent} onStop={stopAgent} requests={trailingRequests} />
          </div>
        ) : null}
        {pendingSends.map((item, index) => (
          <PendingMessageRow
            continued={index > 0 || entries.at(-1)?.message?.author.id === session.user.actorId}
            key={item.key}
            name={session.user.displayName}
            seed={session.user.actorId}
            text={item.text}
          />
        ))}
        {Object.entries(realtime.streaming).map(([messageId, text]) => (
          <article aria-label="Codex is writing" className="aw-msg is-streaming" key={messageId}>
            <div className="aw-msg__gutter">
              <Avatar kind="worker" name="Codex" seed="codex" size={36} working />
            </div>
            <div className="aw-msg__main">
              <div className="aw-msg__meta">
                <span className="aw-msg__author">Codex</span>
                <ActorTag kind="worker" />
                <Dots label="Writing" />
              </div>
              <div className="aw-msg__body">{text}</div>
            </div>
          </article>
        ))}
        {runForCard ? (
          <div className="aw-timeline__card">
            <RunCard actions={runActions} notes={runNotes} run={runForCard} />
          </div>
        ) : null}
        {pendingApproval ? (
          <div className="aw-timeline__card">
            <ApprovalCard
              approval={pendingApproval}
              busyAction={busyAction}
              canRespond={pendingApproval.capabilities.respond}
              onRespond={respond}
            />
          </div>
        ) : null}
      </ConversationTimeline>

      {mode === 'message' && canMessage ? (
        <StructuredComposer
          label={`Message ${threadName}`}
          onSubmit={sendMessage}
          placeholder={`Reply in ${threadName}`}
          searchActors={searchActors}
          searchWorkItems={searchWorkItems}
          toolbar={modeSwitch}
        />
      ) : null}
      {mode === 'run' && showRunComposer ? (
        <form className="aw-composer-wrap" onSubmit={startRun}>
          {modeSwitch ? <div className="aw-composer__toolbar">{modeSwitch}</div> : null}
          <div className="aw-composer aw-composer--run">
            <label className="visually-hidden" htmlFor="agent-prompt">
              Describe the change for Codex
            </label>
            <textarea
              disabled={Boolean(activeRun) || busyAction === 'start'}
              id="agent-prompt"
              onChange={(event) => {
                setPrompt(event.target.value)
                setStartIdempotencyKey(null)
              }}
              placeholder={
                activeRun ? 'Codex is already working in this thread. Stop the run to start a new one.' : 'Describe the change for Codex to make…'
              }
              rows={3}
              value={prompt}
            />
            <div className="aw-composer__bar">
              <span className="aw-composer__meta">
                <Icon name="terminal" size={14} /> Runs in an isolated workspace on your host
              </span>
              <div className="aw-composer__end">
                {messageTextScalarValues(prompt) > 18_000 ? (
                  <span className="aw-composer__count">{(20_000 - messageTextScalarValues(prompt)).toLocaleString()}</span>
                ) : null}
                <Button
                  busy={busyAction === 'start'}
                  disabled={Boolean(activeRun) || !prompt.trim() || !isSafeMessageText(prompt.trim())}
                  icon="terminal"
                  size="sm"
                  type="submit"
                  variant="primary"
                >
                  {busyAction === 'start' ? 'Starting…' : 'Start run'}
                </Button>
              </div>
            </div>
          </div>
          <p className="aw-composer__hint" aria-hidden="true" />
        </form>
      ) : null}
      {verifiedTimeline && archived ? (
        <StructuredComposer disabledReason="This thread is archived. Nobody can post here." onSubmit={async () => undefined} searchActors={async () => []} />
      ) : null}

      {messageActions.dialogs}
      <Dialog
        description="People can still read it from Archived conversations, but no one can post or start runs."
        footer={
          <>
            <Button onClick={() => setArchiveOpen(false)}>Cancel</Button>
            <Button
              busy={archiveThread.isPending}
              onClick={() => {
                if (!verifiedTimeline) return
                archiveThread.mutate(verifiedTimeline.conversation.version ?? 1, { onSettled: () => setArchiveOpen(false) })
              }}
              variant="danger"
            >
              Archive thread
            </Button>
          </>
        }
        onClose={() => setArchiveOpen(false)}
        open={archiveOpen}
        title={`Archive “${threadName}”?`}
      />
    </AppShell>
  )
}

import {
  FormEvent,
  useEffect,
  useMemo,
  useRef,
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
import { ConversationTimeline, OlderMessagesButton, PendingMessageRow, TimelineEntries } from '../conversations/Timeline'
import {
  archiveConversation,
  markConversationSeen,
  normalizeTimelinePage,
  searchMentionableActors,
  sendConversationMessage,
} from '../conversations/api'
import type {
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
  capabilities: {
    workerInvoke: boolean
    view: boolean
    sendMessage: boolean
    archive: boolean
    markSeen: boolean
  }
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
  const [busyAction, setBusyAction] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [acknowledgedRunId, setAcknowledgedRunId] = useState<string | null>(null)
  const [olderEntries, setOlderEntries] = useState<{
    threadId: string
    entries: ConversationTimelineEntry[]
    nextCursor: string | null
  }>({ threadId: '', entries: [], nextCursor: null })
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [detailsOpen, setDetailsOpen] = useState(false)
  const [pendingSends, setPendingSends] = useState<{ key: string; text?: string }[]>([])
  const [archiveOpen, setArchiveOpen] = useState(false)
  const [composerMode, setComposerMode] = useState<'message' | 'run'>('message')
  const parentSidebar = useProjectConversationSidebar(projectId, verifiedTimeline?.conversation.parentChannelId ?? '')
  const markedSeen = useRef({ threadId: '', sequence: 0 })
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
  const entries = useMemo(() => {
    const bySequence = new Map<number, ConversationTimelineEntry>()
    const historical =
      olderEntries.threadId === threadId ? olderEntries.entries : []
    for (const entry of [...historical, ...(verifiedTimeline?.entries ?? [])]) {
      bySequence.set(entry.sequence, entry)
    }
    return [...bySequence.values()].sort(
      (left, right) => left.sequence - right.sequence,
    )
  }, [olderEntries, threadId, verifiedTimeline?.entries])
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
  const olderCursor =
    olderEntries.threadId === threadId
      ? olderEntries.nextCursor
      : verifiedTimeline?.nextCursor ?? null

  useEffect(() => {
    if (
      !verifiedTimeline?.capabilities.markSeen ||
      verifiedTimeline.highestSequence <=
        verifiedTimeline.readState.highestSeenSequence ||
      (markedSeen.current.threadId === threadId &&
        verifiedTimeline.highestSequence <= markedSeen.current.sequence) ||
      entries.at(-1)?.sequence !== verifiedTimeline.highestSequence
    ) {
      return
    }
    markedSeen.current = {
      threadId,
      sequence: verifiedTimeline.highestSequence,
    }
    void markConversationSeen({
      conversationId: threadId,
      highestSeenSequence: verifiedTimeline.highestSequence,
    })
      .then(() =>
        queryClient.invalidateQueries({
          queryKey: queryKeys.conversationSidebarRoot(projectId),
        }),
      )
      .catch(() => {
        markedSeen.current = {
          threadId,
          sequence: verifiedTimeline.readState.highestSeenSequence,
        }
      })
  }, [entries, projectId, queryClient, threadId, verifiedTimeline])

  const perform = async (key: string, action: () => Promise<unknown>) => {
    setBusyAction(key)
    setActionError(null)
    try {
      await action()
      setPrompt((current) => (key === 'start' ? '' : current))
      await thread.timeline.refetch()
      return true
    } catch (reason) {
      setActionError(reason instanceof Error ? reason.message : 'Action failed.')
      return false
    } finally {
      setBusyAction(null)
    }
  }

  const startRun = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (verifiedTimeline?.capabilities.workerInvoke !== true || activeRun) {
      return
    }
    const value = prompt.trim()
    if (!value || !isSafeMessageText(value)) return
    const commandKey = startIdempotencyKey ?? crypto.randomUUID()
    if (!startIdempotencyKey) setStartIdempotencyKey(commandKey)
    void perform('start', () =>
      requestJson(
        `/api/v1/threads/${encodeURIComponent(threadId)}/agent-runs`,
        {
          method: 'POST',
          idempotencyKey: commandKey,
          json: { prompt: value },
        },
      ),
    ).then((succeeded) => {
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
    const action = 'decision' in response ? response.decision : 'answer'
    void perform(`${approvalId}:${action}`, () =>
      requestJson(
        `/api/v1/agent-approvals/${encodeURIComponent(approvalId)}/respond`,
        {
          method: 'POST',
          idempotencyKey: `approval-${approvalId}-${action}`,
          json: response,
        },
      ),
    )
  }

  const interrupt = () => {
    const runId = activeRun?.id
    if (!runId || activeRun.capabilities.interrupt !== true) return
    void perform('interrupt', () =>
      requestJson(
        `/api/v1/agent-runs/${encodeURIComponent(runId)}/interrupt`,
        { method: 'POST', idempotencyKey: `interrupt-${runId}`, json: {} },
      ),
    )
  }

  const retryLastRun = () => {
    const runId = latestTerminalRun?.id
    if (!runId || latestTerminalRun.capabilities.retry !== true || activeRun) return
    void perform('retry', () =>
      requestJson(`/api/v1/agent-runs/${encodeURIComponent(runId)}/retry`, {
        method: 'POST',
        idempotencyKey: `retry-${runId}`,
        json: {},
      }),
    )
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
    void perform('publish', () =>
      requestJson(
        `/api/v1/projects/${encodeURIComponent(projectId)}/agent-runs/${encodeURIComponent(runId)}/publish`,
        {
          method: 'POST',
          idempotencyKey: commandKey,
          json: {},
        },
      ),
    ).then((succeeded) => {
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
      requestJson(
        `/api/v1/projects/${encodeURIComponent(projectId)}/agent-runs/${encodeURIComponent(runId)}/pull-request`,
        {
          method: 'POST',
          idempotencyKey: commandKey,
          json: {
            title,
            body: 'Created from an explicitly published Agent Workspace run.',
            workItemIds:
              verifiedTimeline?.linkedWorkItems.map((item) => item.id) ?? [],
          },
        },
      ),
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
    void perform('pull-request-refresh', () =>
      requestJson(
        `/api/v1/projects/${encodeURIComponent(projectId)}/pull-requests/${encodeURIComponent(pullRequestId)}/refresh`,
        {
          method: 'POST',
          idempotencyKey: crypto.randomUUID(),
          json: {},
        },
      ),
    )
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
    void perform('acknowledge-failure', () =>
      requestJson(
        `/api/v1/agent-runs/${encodeURIComponent(runId)}/acknowledge-failure`,
        {
          method: 'POST',
          idempotencyKey: `acknowledge-failure-${runId}-${latestFailureEvent.id}`,
          json: { eventId: latestFailureEvent.id },
        },
      ),
    ).then((succeeded) => {
      if (succeeded) setAcknowledgedRunId(runId)
    })
  }

  const loadOlder = async () => {
    if (!olderCursor || loadingOlder) return
    setLoadingOlder(true)
    setActionError(null)
    try {
      const parameters = new URLSearchParams({
        limit: '50',
        cursor: olderCursor,
      })
      if (includeArchived) parameters.set('includeArchived', 'true')
      const older = normalizeTimeline(
        await requestJson<unknown>(
          `/api/v1/conversations/${encodeURIComponent(threadId)}/timeline?${parameters}`,
        ),
      )
      if (
        older.conversation.id !== threadId ||
        older.conversation.projectId !== projectId ||
        older.conversation.kind !== 'thread'
      ) {
        throw new Error('Older timeline page crossed its Project boundary.')
      }
      setOlderEntries((current) => ({
        threadId,
        entries: [
          ...older.entries,
          ...(current.threadId === threadId ? current.entries : []),
        ],
        nextCursor: older.nextCursor,
      }))
    } catch (reason) {
      setActionError(
        reason instanceof Error ? reason.message : 'Older Messages could not load.',
      )
    } finally {
      setLoadingOlder(false)
    }
  }

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
      mainClassName="aw-main--conversation"
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
        {olderCursor ? <OlderMessagesButton busy={loadingOlder} onClick={() => void loadOlder()} /> : null}
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
        <TimelineEntries entries={entries} viewerId={session.user.actorId} />
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
        {actionError ? (
          <div className="aw-timeline__card">
            <Banner tone="danger">{actionError}</Banner>
          </div>
        ) : null}
      </ConversationTimeline>

      {mode === 'message' && canMessage ? (
        <StructuredComposer
          label={`Message ${threadName}`}
          onSubmit={sendMessage}
          placeholder={`Reply in ${threadName}`}
          searchActors={async (query) => (await searchMentionableActors(projectId, query)).actors}
          searchWorkItems={(query) => searchWorkItemReferences(projectId, query)}
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

      <Dialog
        description="People can still read it from Archived conversations, but no one can post or start runs."
        footer={
          <>
            <Button onClick={() => setArchiveOpen(false)}>Cancel</Button>
            <Button
              onClick={() => {
                if (!verifiedTimeline) return
                void archiveConversation({
                  conversationId: threadId,
                  kind: 'thread',
                  expectedVersion: verifiedTimeline.conversation.version ?? 1,
                }).then(() => {
                  setArchiveOpen(false)
                  navigate(channelRoute, { replace: true })
                })
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

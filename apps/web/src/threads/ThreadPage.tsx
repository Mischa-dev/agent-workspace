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
} from '../projects/ProjectOverviewPage'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { MessageContent } from '../conversations/MessageContent'
import {
  isSafeMessageText,
  messageTextScalarValues,
} from '../conversations/messageContentModel'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { SystemTimelineEntry } from '../conversations/SystemTimelineEntry'
import type { ComposerSubmission } from '../conversations/composerModel'
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

type RunState =
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

type Run = {
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
  pullRequestUrl: string | null
  pullRequestTitle: string | null
  pullRequestState: string | null
  pullRequestDraft: boolean | null
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
  }
}

type Approval = {
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

const stateLabels: Record<RunState, string> = {
  created: 'Created',
  queued: 'Queued',
  starting: 'Starting',
  working: 'Working',
  blocked: 'Needs input',
  recovering: 'Recovering',
  completed: 'Completed',
  failed: 'Failed',
  canceled: 'Canceled',
  interrupted: 'Interrupted',
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
          pullRequestUrl: null,
          pullRequestTitle: null,
          pullRequestState: null,
          pullRequestDraft: null,
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
    await sendConversationMessage({
      conversationId: threadId,
      kind: 'thread',
      ...submission,
    })
    await thread.timeline.refetch()
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
  const context = verifiedTimeline ? (
    <ThreadContext
      projectName={project.data.name}
      run={activeRun}
      threadId={threadId}
      workItems={verifiedTimeline.linkedWorkItems}
      onUnlink={verifiedTimeline.capabilities.sendMessage ? (workItemId) => {
        workItemLink.mutate(
          { projectId, workItemId, threadId, linked: false, idempotencyKey: crypto.randomUUID() },
          { onSuccess: () => void thread.timeline.refetch() },
        )
      } : undefined}
      unlinking={workItemLink.isPending}
    />
  ) : undefined
  const showComposer = Boolean(
    verifiedTimeline &&
      (activeRun
        ? activeRun.capabilities.interrupt
        : verifiedTimeline.capabilities.workerInvoke),
  )

  return (
    <AppShell
      context={context}
      mainClassName="workspace thread-workspace"
      project={project.data}
      selectedChannelId={
        verifiedTimeline?.conversation.parentChannelId ?? undefined
      }
      thread={
        verifiedTimeline ? { id: threadId, name: threadName } : null
      }
      user={session.user}
    >
      <NavigationSyncAlert {...navigationSync} />
      <header className="workspace-heading">
        <div>
          <span className="section-label">Thread</span>
          <h1 id="workspace-heading">{threadName}</h1>
        </div>
        {verifiedTimeline && (
          <div className="header-status">
            <span
              className="connection-state"
              data-connection={realtime.connection}
            >
              <span aria-hidden="true">●</span>{' '}
              {realtime.connection === 'live' ? 'Live' : 'Reconnecting'}
            </span>
            <RunBadge run={activeRun} />
            {verifiedTimeline.capabilities.archive &&
            !verifiedTimeline.conversation.archivedAt ? (
              <button
                className="secondary-button"
                onClick={() => {
                  void archiveConversation({
                    conversationId: threadId,
                    kind: 'thread',
                    expectedVersion: verifiedTimeline.conversation.version ?? 1,
                  }).then(() => {
                    const parent = verifiedTimeline.conversation.parentChannelId
                    navigate(
                      parent
                        ? `/projects/${encodeURIComponent(projectId)}/channels/${encodeURIComponent(parent)}`
                        : `/projects/${encodeURIComponent(projectId)}`,
                      { replace: true },
                    )
                  })
                }}
                type="button"
              >
                Archive Thread
              </button>
            ) : null}
          </div>
        )}
      </header>

      <div
        aria-busy={thread.timeline.isPending}
        className="timeline"
        data-empty={!verifiedTimeline?.entries.length}
      >
        {olderCursor ? (
          <button
            className="load-older-button"
            disabled={loadingOlder}
            onClick={() => void loadOlder()}
            type="button"
          >
            {loadingOlder ? 'Loading…' : 'Load older Messages'}
          </button>
        ) : null}
        {thread.timeline.isPending && <TimelineSkeleton />}
        {thread.timeline.isError && (
          <div className="error-state" role="alert">
            <h2>Thread unavailable</h2>
            <p>
              The Thread does not exist in this Project or could not be loaded.
            </p>
            <button
              onClick={() => void thread.timeline.refetch()}
              type="button"
            >
              Try again
            </button>
          </div>
        )}
        {verifiedTimeline && !verifiedTimeline.entries.length && (
            <div className="empty-state">
              <span className="empty-state-mark" aria-hidden="true">
                ◌
              </span>
              <h2>Start the first hosted run.</h2>
              <p>
                The prompt, approval, significant provider events, and final
                worker response will be kept in this durable Thread.
              </p>
            </div>
          )}
        {entries.map((entry) => (
          <TimelineItem entry={entry} key={entry.id} />
        ))}
        {Object.entries(realtime.streaming).map(([messageId, text]) => (
          <article
            className="message worker-message streaming-message"
            key={messageId}
          >
            <header>
              <span className="avatar" aria-hidden="true">
                A
              </span>
              <strong>@agent</strong>
              <span>Streaming</span>
            </header>
            <p>{text}</p>
          </article>
        ))}
        {pendingApproval && (
          <ApprovalPrompt
            approval={pendingApproval}
            busyAction={busyAction}
            canRespond={pendingApproval.capabilities.respond}
            onRespond={respond}
          />
        )}
        <p aria-live="polite" className="visually-hidden">
          {actionError}
        </p>
      </div>

      {verifiedTimeline?.capabilities.sendMessage &&
      !verifiedTimeline.conversation.archivedAt ? (
        <StructuredComposer
          onSubmit={sendMessage}
          searchActors={async (query) =>
            (await searchMentionableActors(projectId, query)).actors
          }
          searchWorkItems={(query) => searchWorkItemReferences(projectId, query)}
        />
      ) : null}

      {showComposer && (
        <form className="composer" onSubmit={startRun}>
          <label htmlFor="agent-prompt">Ask the execution worker</label>
          <textarea
            disabled={Boolean(activeRun) || busyAction === 'start'}
            id="agent-prompt"
            onChange={(event) => {
              setPrompt(event.target.value)
              setStartIdempotencyKey(null)
            }}
            placeholder={
              activeRun
                ? 'This Thread already has an active run.'
                : 'Describe the isolated change to make…'
            }
            rows={3}
            value={prompt}
          />
          <div className="composer-actions">
            <span>
              {messageTextScalarValues(prompt).toLocaleString()} / 20,000
            </span>
            {activeRun ? (
              <button
                className="danger-button"
                disabled={busyAction === 'interrupt'}
                onClick={interrupt}
                type="button"
              >
                {busyAction === 'interrupt'
                  ? 'Interrupting…'
                  : 'Interrupt run'}
              </button>
            ) : (
              <>
                {latestTerminalRun?.capabilities.publish &&
                  latestTerminalRun.repositoryWorkspaceId && (
                    <button
                      className="secondary-button"
                      disabled={Boolean(busyAction)}
                      onClick={publishLastRun}
                      type="button"
                    >
                      {busyAction === 'publish'
                        ? 'Queueing publish…'
                        : 'Publish branch'}
                    </button>
                  )}
                {latestTerminalRun?.capabilities.createPullRequest && (
                  <button
                    className="secondary-button"
                    disabled={Boolean(busyAction)}
                    onClick={createPullRequest}
                    type="button"
                  >
                    {busyAction === 'pull-request'
                      ? 'Queueing pull request…'
                      : 'Create pull request'}
                  </button>
                )}
                {latestTerminalRun?.capabilities.retry && (
                  <button
                    className="secondary-button"
                    disabled={Boolean(busyAction)}
                    onClick={retryLastRun}
                    type="button"
                  >
                    {busyAction === 'retry' ? 'Retrying…' : 'Retry last run'}
                  </button>
                )}
                {latestFailureEvent &&
                  !latestTerminalRun?.failureAcknowledged &&
                  acknowledgedRunId !== latestTerminalRun?.id && (
                    <button
                      className="secondary-button"
                      disabled={Boolean(busyAction)}
                      onClick={acknowledgeFailure}
                      type="button"
                    >
                      {busyAction === 'acknowledge-failure'
                        ? 'Acknowledging…'
                        : 'Acknowledge failure'}
                    </button>
                  )}
                <button
                  className="primary-button"
                  disabled={
                    !prompt.trim() ||
                    !isSafeMessageText(prompt.trim()) ||
                    busyAction === 'start'
                  }
                  type="submit"
                >
                  {busyAction === 'start' ? 'Starting…' : 'Start run'}
                </button>
              </>
            )}
          </div>
          {latestTerminalRun?.publishedCommit && (
            <p className="compact-note">
              Published commit{' '}
              <code>{latestTerminalRun.publishedCommit.slice(0, 12)}</code>
              {latestTerminalRun.publishedAt
                ? ` at ${new Date(latestTerminalRun.publishedAt).toLocaleString()}`
                : ''}
            </p>
          )}
          {latestTerminalRun?.publishFailureCategory && (
            <p className="action-error" role="status">
              Publish needs attention:{' '}
              {latestTerminalRun.publishFailureCategory.replaceAll('_', ' ')}.
            </p>
          )}
          {latestTerminalRun?.pullRequestUrl &&
            latestTerminalRun.pullRequestNumber && (
              <p className="compact-note">
                Pull request{' '}
                <a
                  href={latestTerminalRun.pullRequestUrl}
                  rel="noreferrer"
                  target="_blank"
                >
                  #{latestTerminalRun.pullRequestNumber}{' '}
                  {latestTerminalRun.pullRequestTitle}
                </a>{' '}
                · {latestTerminalRun.pullRequestDraft ? 'draft ' : ''}
                {latestTerminalRun.pullRequestState}
              </p>
            )}
          {latestTerminalRun?.pullRequestRequestState &&
            !latestTerminalRun.pullRequestUrl && (
              <p className="compact-note">
                Pull request: {latestTerminalRun.pullRequestRequestState}
              </p>
            )}
          {latestTerminalRun?.pullRequestFailureCategory && (
            <p className="action-error" role="status">
              Pull request needs attention:{' '}
              {latestTerminalRun.pullRequestFailureCategory.replaceAll(
                '_',
                ' ',
              )}
              .
            </p>
          )}
          {actionError && (
            <p className="action-error" role="alert">
              {actionError}
            </p>
          )}
        </form>
      )}
    </AppShell>
  )
}

function shortId(value: string | null | undefined) {
  return value ? `${value.slice(0, 8)}…` : '—'
}

function RunBadge({ run }: { run: Run | null }) {
  if (!run)
    return (
      <span className="status-chip" data-state="settled">
        Ready
      </span>
    )
  return (
    <span className="status-chip" data-state={run.state}>
      <span aria-hidden="true">{run.state === 'blocked' ? '!' : '●'}</span>
      {stateLabels[run.state]}
      {run.state === 'queued' && run.queuePosition
        ? ` · ${run.queuePosition} in queue`
        : null}
    </span>
  )
}

function TimelineItem({ entry }: { entry: ConversationTimelineEntry }) {
  if (entry.message) {
    const worker = entry.message.author.kind === 'execution_worker'
    return (
      <article className={`message ${worker ? 'worker-message' : ''}`}>
        <header>
          <span className="avatar" aria-hidden="true">
            {worker ? 'A' : 'M'}
          </span>
          <strong>@{entry.message.author.handle}</strong>
          <time dateTime={entry.createdAt}>
            {new Date(entry.createdAt).toLocaleTimeString([], {
              hour: 'numeric',
              minute: '2-digit',
            })}
          </time>
        </header>
        <p>
          <MessageContent
            actors={entry.message.mentionedActors ?? []}
            content={entry.message.content}
            fallbackPlainText={entry.message.plainText}
            workItems={entry.message.referencedWorkItems ?? []}
          />
        </p>
      </article>
    )
  }
  if (entry.workerEvent) {
    return (
      <article className="provider-event" data-kind={entry.workerEvent.kind}>
        <span className="event-icon" aria-hidden="true">
          {eventIcon(entry.workerEvent.kind)}
        </span>
        <div>
          <strong>{eventLabel(entry.workerEvent.kind)}</strong>
          <p>{entry.workerEvent.summary}</p>
        </div>
        <time dateTime={entry.createdAt}>
          {new Date(entry.createdAt).toLocaleTimeString([], {
            hour: 'numeric',
            minute: '2-digit',
          })}
        </time>
      </article>
    )
  }
  return <SystemTimelineEntry createdAt={entry.createdAt} />
}

function eventIcon(kind: string) {
  if (kind.includes('file')) return 'Δ'
  if (kind.includes('approval') || kind.includes('question')) return '?'
  if (kind.includes('interrupt') || kind.includes('failed')) return '×'
  if (kind.includes('completed')) return '✓'
  if (kind.includes('recovery')) return '↻'
  return '·'
}

function eventLabel(kind: string) {
  return kind
    .split('_')
    .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
    .join(' ')
}

function ApprovalPrompt({
  approval,
  busyAction,
  canRespond,
  onRespond,
}: {
  approval: Approval
  busyAction: string | null
  canRespond: boolean
  onRespond: (
    approvalId: string,
    response: { decision: 'approve' | 'deny' } | { answer: string },
  ) => void
}) {
  const [answer, setAnswer] = useState('')
  const answerAction = `${approval.id}:answer`
  return (
    <section aria-labelledby={`approval-${approval.id}`} className="approval-prompt">
      <div className="approval-symbol" aria-hidden="true">
        ?
      </div>
      <div>
        <h2 id={`approval-${approval.id}`}>
          {approval.kind === 'question'
            ? 'Worker question'
            : 'Approval required'}
        </h2>
        <p>{approval.summary}</p>
        {canRespond && approval.kind === 'question' && (
          <form
            className="approval-actions"
            onSubmit={(event) => {
              event.preventDefault()
              const value = answer.trim()
              if (value) onRespond(approval.id, { answer: value })
            }}
          >
            <label htmlFor={`approval-answer-${approval.id}`}>Your answer</label>
            {approval.options.length > 0 && (
              <div aria-label="Suggested answers" className="approval-actions">
                {approval.options.map((option) => (
                  <button
                    className="secondary-button"
                    disabled={Boolean(busyAction)}
                    key={option.id}
                    onClick={() => setAnswer(option.label)}
                    type="button"
                  >
                    {option.label}
                  </button>
                ))}
              </div>
            )}
            <textarea
              disabled={Boolean(busyAction)}
              id={`approval-answer-${approval.id}`}
              maxLength={4000}
              onChange={(event) => setAnswer(event.target.value)}
              required
              rows={3}
              value={answer}
            />
            <button
              className="primary-button"
              disabled={Boolean(busyAction) || !answer.trim()}
              type="submit"
            >
              {busyAction === answerAction ? 'Sending…' : 'Send answer'}
            </button>
          </form>
        )}
        {canRespond && approval.kind === 'approval' && (
          <div className="approval-actions">
            <button
              className="primary-button"
              disabled={Boolean(busyAction)}
              onClick={() => onRespond(approval.id, { decision: 'approve' })}
              type="button"
            >
              {busyAction === `${approval.id}:approve`
                ? 'Allowing…'
                : 'Allow once'}
            </button>
            <button
              className="secondary-button"
              disabled={Boolean(busyAction)}
              onClick={() => onRespond(approval.id, { decision: 'deny' })}
              type="button"
            >
              {busyAction === `${approval.id}:deny` ? 'Denying…' : 'Deny'}
            </button>
          </div>
        )}
      </div>
    </section>
  )
}

function TimelineSkeleton() {
  return (
    <div aria-label="Loading Thread" className="timeline-skeleton">
      <span />
      <span />
      <span />
    </div>
  )
}

function ThreadContext({
  projectName,
  run,
  threadId,
  workItems,
  onUnlink,
  unlinking,
}: {
  projectName: string
  run: Run | null
  threadId: string
  workItems: ConversationWorkItemReference[]
  onUnlink?: (workItemId: string) => void
  unlinking: boolean
}) {
  return (
    <aside aria-label="Execution context" className="context-drawer">
      <span className="section-label">Execution context</span>
      <h2>Project host</h2>
      <dl>
        <div>
          <dt>Project</dt>
          <dd>{projectName}</dd>
        </div>
        <div>
          <dt>Provider</dt>
          <dd>Codex App Server</dd>
        </div>
        <div>
          <dt>Thread</dt>
          <dd>{shortId(threadId)}</dd>
        </div>
        <div>
          <dt>Run</dt>
          <dd>{shortId(run?.id)}</dd>
        </div>
        <div>
          <dt>Workspace</dt>
          <dd>{shortId(run?.repositoryWorkspaceId)}</dd>
        </div>
        <div>
          <dt>Branch</dt>
          <dd>{run?.branch ?? 'Allocated after admission'}</dd>
        </div>
        <div>
          <dt>Base commit</dt>
          <dd>{shortId(run?.baseCommit)}</dd>
        </div>
      </dl>
      <div className="thread-linked-work-items">
        <h3>Linked Work Items</h3>
        {workItems.length ? (
          <ul>
            {workItems.map((item) => (
              <li key={item.id}>
                <div>
                  <Link to={item.href}>{item.key}</Link>
                  <span>{item.title}</span>
                  {item.archivedAt ? <small>Archived</small> : null}
                </div>
                {onUnlink ? <button className="text-button" disabled={unlinking} onClick={() => onUnlink(item.id)} type="button">Unlink</button> : null}
              </li>
            ))}
          </ul>
        ) : <p className="compact-note">No linked Work Items.</p>}
      </div>
      <p className="context-note">
        Authentication and authorship are resolved from the server-side session.
      </p>
    </aside>
  )
}

import {
  FormEvent,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { useQuery } from '@tanstack/react-query'
import { Navigate, useParams } from 'react-router-dom'

import { requestJson } from '../api/client'
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
import { coalesceDeltaBatch } from '../app/streaming'

type Actor = {
  id: string
  kind: string
  displayName: string
  handle: string
}

type Message = {
  id: string
  author: Actor
  content: unknown
  plainText: string
}

type WorkerEvent = {
  id: string
  agentRunId: string
  kind: string
  summary: string
  payload: Record<string, unknown>
}

type TimelineEntry = {
  id: string
  sequence: number
  kind: 'message' | 'worker_event' | 'system'
  createdAt: string
  message: Message | null
  workerEvent: WorkerEvent | null
}

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
  failureReason: string | null
  createdAt: string
  updatedAt: string
  capabilities: {
    interrupt: boolean
  }
}

type Approval = {
  id: string
  agentRunId: string
  kind: 'approval' | 'question'
  summary: string
  options: Array<{ id: string; label: string }>
  status: string
  response: { decision?: string } | null
  createdAt: string
  capabilities: {
    respond: boolean
  }
}

type Timeline = {
  conversation: {
    id: string
    projectId: string
    kind: string
    parentChannelId: string | null
    name: string
  }
  entries: TimelineEntry[]
  activeRun: Run | null
  approvals: Approval[]
  capabilities: {
    workerInvoke: boolean
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

function websocketUrl() {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${protocol}//${window.location.host}/api/v1/realtime`
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
  const conversation = asRecord(record.conversation)
  if (
    typeof conversation.id !== 'string' ||
    typeof conversation.projectId !== 'string'
  ) {
    throw new Error('Timeline response is missing its conversation boundary.')
  }
  const activeRunRecord = asRecord(record.activeRun)
  const activeRun =
    typeof activeRunRecord.id === 'string'
      ? ({
          ...activeRunRecord,
          capabilities: {
            interrupt: explicitCapability(
              activeRunRecord.capabilities,
              'interrupt',
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
    conversation: conversation as Timeline['conversation'],
    entries: Array.isArray(record.entries)
      ? (record.entries as TimelineEntry[])
      : [],
    activeRun,
    approvals,
    capabilities: {
      workerInvoke: explicitCapability(
        record.capabilities,
        'workerInvoke',
      ),
    },
  }
}

function useThread(projectId: string, threadId: string) {
  const timeline = useQuery({
    queryKey: queryKeys.timeline(projectId, threadId),
    queryFn: async () =>
      normalizeTimeline(
        await requestJson<unknown>(
          `/api/v1/conversations/${encodeURIComponent(threadId)}/timeline`,
        ),
      ),
    enabled: Boolean(threadId),
    retry: false,
  })
  const projectMatches = timeline.data?.conversation.projectId === projectId
  const [streamingState, setStreamingState] = useState<{
    threadId: string
    messages: Record<string, string>
  }>({ threadId: '', messages: {} })
  const [connectionState, setConnectionState] = useState<{
    threadId: string
    status: 'connecting' | 'live' | 'retrying'
  }>({ threadId: '', status: 'connecting' })
  const reconnectAttempt = useRef(0)

  useEffect(() => {
    if (!threadId || !projectMatches) return
    let disposed = false
    let socket: WebSocket | undefined
    let retry: number | undefined
    let streamFrame: number | undefined
    let pendingDeltas: Record<string, string> = {}

    const queueDelta = (messageId: string, delta: string) => {
      pendingDeltas[messageId] = `${pendingDeltas[messageId] ?? ''}${delta}`
      if (streamFrame !== undefined) return
      streamFrame = window.requestAnimationFrame(() => {
        const batch = pendingDeltas
        pendingDeltas = {}
        streamFrame = undefined
        setStreamingState((current) => ({
          threadId,
          messages: coalesceDeltaBatch(
            current.threadId === threadId ? current.messages : {},
            batch,
          ),
        }))
      })
    }

    const connect = () => {
      if (disposed) return
      setConnectionState({
        threadId,
        status:
          reconnectAttempt.current === 0 ? 'connecting' : 'retrying',
      })
      socket = new WebSocket(websocketUrl())
      socket.addEventListener('open', () => {
        reconnectAttempt.current = 0
        setConnectionState({ threadId, status: 'live' })
        setStreamingState({ threadId, messages: {} })
        void timeline.refetch()
      })
      socket.addEventListener('message', (event) => {
        let payload: {
          kind?: string
          conversation_id?: string
          conversationId?: string
          payload?: { message_id?: string; delta?: string }
        }
        try {
          payload = JSON.parse(String(event.data)) as typeof payload
        } catch {
          return
        }
        const conversationId =
          payload.conversation_id ?? payload.conversationId
        if (conversationId !== threadId) return
        if (
          payload.kind === 'message.delta' &&
          payload.payload?.message_id &&
          typeof payload.payload.delta === 'string'
        ) {
          queueDelta(payload.payload.message_id, payload.payload.delta)
          return
        }
        setStreamingState({ threadId, messages: {} })
        void timeline.refetch()
      })
      socket.addEventListener('close', () => {
        if (disposed) return
        reconnectAttempt.current += 1
        setConnectionState({ threadId, status: 'retrying' })
        void timeline.refetch()
        const delay = Math.min(1_000 * 2 ** reconnectAttempt.current, 10_000)
        retry = window.setTimeout(connect, delay)
      })
    }

    connect()
    return () => {
      disposed = true
      if (retry) window.clearTimeout(retry)
      if (streamFrame !== undefined) window.cancelAnimationFrame(streamFrame)
      socket?.close()
    }
    // The query's stable refetch function is intentionally scoped to this Thread.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectMatches, threadId])

  return {
    connection:
      connectionState.threadId === threadId
        ? connectionState.status
        : 'connecting',
    projectMatches,
    streaming:
      projectMatches && streamingState.threadId === threadId
        ? streamingState.messages
        : {},
    timeline,
  }
}

export function ThreadPage() {
  const session = useAuthenticatedSession()
  const { projectId = '', threadId = '' } = useParams()
  const project = useProject(projectId)
  const thread = useThread(projectId, threadId)
  const timeline = thread.timeline.data
  const verifiedTimeline = thread.projectMatches ? timeline : undefined
  const activeRun = verifiedTimeline?.activeRun ?? null
  const [prompt, setPrompt] = useState('')
  const [busyAction, setBusyAction] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'thread', conversationId: threadId },
    project.isSuccess &&
      project.data.capabilities.navigationWrite &&
      thread.timeline.isSuccess &&
      thread.projectMatches,
    project.data?.navigation,
  )

  const pendingApproval = useMemo(
    () =>
      verifiedTimeline?.approvals.find(
        (approval) => approval.status === 'pending',
      ),
    [verifiedTimeline],
  )

  const perform = async (key: string, action: () => Promise<unknown>) => {
    setBusyAction(key)
    setActionError(null)
    try {
      await action()
      setPrompt((current) => (key === 'start' ? '' : current))
      await thread.timeline.refetch()
    } catch (reason) {
      setActionError(reason instanceof Error ? reason.message : 'Action failed.')
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
    if (!value) return
    void perform('start', () =>
      requestJson(`/api/v1/threads/${encodeURIComponent(threadId)}/agent-runs`, {
        method: 'POST',
        json: { prompt: value },
      }),
    )
  }

  const respond = (approvalId: string, decision: 'approve' | 'deny') => {
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
    void perform(`${approvalId}:${decision}`, () =>
      requestJson(
        `/api/v1/agent-approvals/${encodeURIComponent(approvalId)}/respond`,
        {
          method: 'POST',
          json: { decision },
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
        { method: 'POST', json: {} },
      ),
    )
  }

  if (thread.timeline.isSuccess && !thread.projectMatches) {
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
              data-connection={thread.connection}
            >
              <span aria-hidden="true">●</span>{' '}
              {thread.connection === 'live' ? 'Live' : 'Reconnecting'}
            </span>
            <RunBadge run={activeRun} />
          </div>
        )}
      </header>

      <div
        aria-busy={thread.timeline.isPending}
        className="timeline"
        data-empty={!verifiedTimeline?.entries.length}
      >
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
        {verifiedTimeline?.entries.map((entry) => (
          <TimelineItem entry={entry} key={entry.id} />
        ))}
        {Object.entries(thread.streaming).map(([messageId, text]) => (
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

      {showComposer && (
        <form className="composer" onSubmit={startRun}>
          <label htmlFor="agent-prompt">Ask the execution worker</label>
          <textarea
            disabled={Boolean(activeRun) || busyAction === 'start'}
            id="agent-prompt"
            maxLength={20_000}
            onChange={(event) => setPrompt(event.target.value)}
            placeholder={
              activeRun
                ? 'This Thread already has an active run.'
                : 'Describe the isolated change to make…'
            }
            rows={3}
            value={prompt}
          />
          <div className="composer-actions">
            <span>{prompt.length.toLocaleString()} / 20,000</span>
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
              <button
                className="primary-button"
                disabled={!prompt.trim() || busyAction === 'start'}
                type="submit"
              >
                {busyAction === 'start' ? 'Starting…' : 'Start run'}
              </button>
            )}
          </div>
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
    </span>
  )
}

function TimelineItem({ entry }: { entry: TimelineEntry }) {
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
        <p>{entry.message.plainText}</p>
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
  return null
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
  onRespond: (approvalId: string, decision: 'approve' | 'deny') => void
}) {
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
        {canRespond && (
          <div className="approval-actions">
            <button
              className="primary-button"
              disabled={Boolean(busyAction)}
              onClick={() => onRespond(approval.id, 'approve')}
              type="button"
            >
              {busyAction === `${approval.id}:approve`
                ? 'Allowing…'
                : 'Allow once'}
            </button>
            <button
              className="secondary-button"
              disabled={Boolean(busyAction)}
              onClick={() => onRespond(approval.id, 'deny')}
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
}: {
  projectName: string
  run: Run | null
  threadId: string
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
      </dl>
      <p className="context-note">
        Authentication and authorship are resolved from the server-side session.
      </p>
    </aside>
  )
}

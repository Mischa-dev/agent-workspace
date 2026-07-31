import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react'

import { coalesceDelta } from './streaming'

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

type Run = {
  id: string
  state: RunState
  repositoryId: string
  repositoryWorkspaceId: string | null
  providerSessionId: string | null
  failureReason: string | null
  createdAt: string
  updatedAt: string
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
}

type Bootstrap = {
  projectId: string
  channelId: string
  threadId: string
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

const projects = [
  { initials: 'AW', label: 'Agent Workspace', active: true },
  { initials: '＋', label: 'Add project', active: false },
]

const channels = ['general', 'implementation', 'research']

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

async function requestJson<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: {
      'content-type': 'application/json',
      ...init?.headers,
    },
  })
  if (!response.ok) {
    const payload = (await response.json().catch(() => null)) as {
      error?: { message?: string }
    } | null
    throw new Error(payload?.error?.message ?? `Request failed (${response.status})`)
  }
  return (await response.json()) as T
}

function useThread() {
  const [bootstrap, setBootstrap] = useState<Bootstrap | null>(null)
  const [timeline, setTimeline] = useState<Timeline | null>(null)
  const [streaming, setStreaming] = useState<Record<string, string>>({})
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [connection, setConnection] = useState<'connecting' | 'live' | 'retrying'>(
    'connecting',
  )
  const reconnectAttempt = useRef(0)

  const loadTimeline = useCallback(async (threadId: string) => {
    const result = await requestJson<Timeline>(
      `/api/v1/conversations/${threadId}/timeline`,
    )
    setTimeline(result)
    setStreaming({})
    setError(null)
  }, [])

  useEffect(() => {
    let cancelled = false
    void requestJson<Bootstrap>('/api/v1/dev/bootstrap')
      .then(async (result) => {
        if (cancelled) return
        setBootstrap(result)
        await loadTimeline(result.threadId)
      })
      .catch((reason: unknown) => {
        if (!cancelled) {
          setError(reason instanceof Error ? reason.message : 'Unable to open Thread.')
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [loadTimeline])

  useEffect(() => {
    if (!bootstrap) return
    let disposed = false
    let socket: WebSocket | undefined
    let retry: number | undefined

    const connect = () => {
      if (disposed) return
      setConnection(reconnectAttempt.current === 0 ? 'connecting' : 'retrying')
      socket = new WebSocket(websocketUrl())
      socket.addEventListener('open', () => {
        reconnectAttempt.current = 0
        setConnection('live')
        void loadTimeline(bootstrap.threadId)
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
        const conversationId = payload.conversation_id ?? payload.conversationId
        if (conversationId !== bootstrap.threadId) return
        if (
          payload.kind === 'message.delta' &&
          payload.payload?.message_id &&
          typeof payload.payload.delta === 'string'
        ) {
          setStreaming((current) =>
            coalesceDelta(
              current,
              payload.payload?.message_id ?? '',
              payload.payload?.delta ?? '',
            ),
          )
          return
        }
        void loadTimeline(bootstrap.threadId)
      })
      socket.addEventListener('close', () => {
        if (disposed) return
        reconnectAttempt.current += 1
        setConnection('retrying')
        const delay = Math.min(1_000 * 2 ** reconnectAttempt.current, 10_000)
        retry = window.setTimeout(connect, delay)
      })
    }

    connect()
    return () => {
      disposed = true
      if (retry) window.clearTimeout(retry)
      socket?.close()
    }
  }, [bootstrap, loadTimeline])

  return {
    bootstrap,
    timeline,
    streaming,
    error,
    loading,
    connection,
    refresh: () => (bootstrap ? loadTimeline(bootstrap.threadId) : Promise.resolve()),
  }
}

export function App() {
  const thread = useThread()
  const [prompt, setPrompt] = useState('')
  const [busyAction, setBusyAction] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)

  const pendingApproval = useMemo(
    () => thread.timeline?.approvals.find((approval) => approval.status === 'pending'),
    [thread.timeline],
  )

  const perform = async (key: string, action: () => Promise<unknown>) => {
    setBusyAction(key)
    setActionError(null)
    try {
      await action()
      await thread.refresh()
    } catch (reason) {
      setActionError(reason instanceof Error ? reason.message : 'Action failed.')
    } finally {
      setBusyAction(null)
    }
  }

  const startRun = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const value = prompt.trim()
    if (!value || !thread.bootstrap) return
    void perform('start', async () => {
      await requestJson(`/api/v1/threads/${thread.bootstrap?.threadId}/agent-runs`, {
        method: 'POST',
        body: JSON.stringify({ prompt: value }),
      })
      setPrompt('')
    })
  }

  const respond = (approvalId: string, decision: 'approve' | 'deny') => {
    void perform(`${approvalId}:${decision}`, () =>
      requestJson(`/api/v1/agent-approvals/${approvalId}/respond`, {
        method: 'POST',
        body: JSON.stringify({ decision }),
      }),
    )
  }

  const interrupt = () => {
    const runId = thread.timeline?.activeRun?.id
    if (!runId) return
    void perform('interrupt', () =>
      requestJson(`/api/v1/agent-runs/${runId}/interrupt`, { method: 'POST' }),
    )
  }

  return (
    <main className="app-shell">
      <nav aria-label="Projects" className="project-rail">
        <div className="brand-mark" aria-label="Agent Workspace">
          A
        </div>
        <div className="project-list">
          {projects.map((project) => (
            <button
              aria-current={project.active ? 'page' : undefined}
              aria-label={project.label}
              className="project-button"
              data-active={project.active}
              key={project.label}
              type="button"
            >
              {project.initials}
            </button>
          ))}
        </div>
      </nav>

      <aside className="project-sidebar">
        <header className="project-heading">
          <span className="section-label">Project</span>
          <h1>Agent Workspace</h1>
        </header>
        <section aria-labelledby="channels-heading">
          <h2 id="channels-heading">Channels</h2>
          <ul>
            {channels.map((channel) => (
              <li key={channel}>
                <button data-selected={channel === 'implementation'} type="button">
                  <span aria-hidden="true">#</span>
                  {channel}
                </button>
                {channel === 'implementation' && (
                  <button
                    aria-current="page"
                    className="thread-link"
                    data-state={thread.timeline?.activeRun?.state ?? 'settled'}
                    type="button"
                  >
                    <span aria-hidden="true">↳</span>
                    M0 walking skeleton
                  </button>
                )}
              </li>
            ))}
          </ul>
        </section>
      </aside>

      <section className="workspace" aria-labelledby="workspace-heading">
        <header className="workspace-heading">
          <div>
            <span className="section-label">Thread in #implementation</span>
            <h2 id="workspace-heading">
              {thread.timeline?.conversation.name ?? 'M0 walking skeleton'}
            </h2>
          </div>
          <div className="header-status">
            <span className="connection-state" data-connection={thread.connection}>
              <span aria-hidden="true">●</span>{' '}
              {thread.connection === 'live' ? 'Live' : 'Reconnecting'}
            </span>
            <RunBadge run={thread.timeline?.activeRun ?? null} />
          </div>
        </header>

        <div
          aria-busy={thread.loading}
          aria-live="polite"
          className="timeline"
          data-empty={!thread.timeline?.entries.length}
        >
          {thread.loading && <TimelineSkeleton />}
          {thread.error && (
            <div className="error-state" role="alert">
              <h3>Thread unavailable</h3>
              <p>{thread.error}</p>
              <button onClick={() => void thread.refresh()} type="button">
                Try again
              </button>
            </div>
          )}
          {!thread.loading && !thread.error && !thread.timeline?.entries.length && (
            <div className="empty-state">
              <span className="empty-state-mark" aria-hidden="true">
                ◌
              </span>
              <h3>Start the first hosted run.</h3>
              <p>
                The prompt, approval, significant provider events, and final worker
                response will be kept in this durable Thread.
              </p>
            </div>
          )}
          {thread.timeline?.entries.map((entry) => (
            <TimelineItem entry={entry} key={entry.id} />
          ))}
          {Object.entries(thread.streaming).map(([messageId, text]) => (
            <article className="message worker-message streaming-message" key={messageId}>
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
              onRespond={respond}
            />
          )}
        </div>

        <form className="composer" onSubmit={startRun}>
          <label htmlFor="agent-prompt">Ask the execution worker</label>
          <textarea
            disabled={Boolean(thread.timeline?.activeRun) || busyAction === 'start'}
            id="agent-prompt"
            maxLength={20_000}
            onChange={(event) => setPrompt(event.target.value)}
            placeholder={
              thread.timeline?.activeRun
                ? 'This Thread already has an active run.'
                : 'Describe the isolated change to make…'
            }
            rows={3}
            value={prompt}
          />
          <div className="composer-actions">
            <span>{prompt.length.toLocaleString()} / 20,000</span>
            {thread.timeline?.activeRun ? (
              <button
                className="danger-button"
                disabled={busyAction === 'interrupt'}
                onClick={interrupt}
                type="button"
              >
                {busyAction === 'interrupt' ? 'Interrupting…' : 'Interrupt run'}
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
      </section>

      <aside className="context-drawer">
        <span className="section-label">Execution context</span>
        <h2>Project host</h2>
        <dl>
          <div>
            <dt>Milestone</dt>
            <dd>M0</dd>
          </div>
          <div>
            <dt>Provider</dt>
            <dd>Codex App Server</dd>
          </div>
          <div>
            <dt>Runtime</dt>
            <dd>Red Hat ARM64</dd>
          </div>
          <div>
            <dt>Thread</dt>
            <dd>{shortId(thread.bootstrap?.threadId)}</dd>
          </div>
          <div>
            <dt>Run</dt>
            <dd>{shortId(thread.timeline?.activeRun?.id)}</dd>
          </div>
          <div>
            <dt>Workspace</dt>
            <dd>{shortId(thread.timeline?.activeRun?.repositoryWorkspaceId)}</dd>
          </div>
        </dl>
        <p className="context-note">
          Development authentication is resolved server-side and is available only
          on a loopback-bound development server.
        </p>
      </aside>
    </main>
  )
}

function shortId(value: string | null | undefined) {
  return value ? `${value.slice(0, 8)}…` : '—'
}

function RunBadge({ run }: { run: Run | null }) {
  if (!run) return <span className="status-chip" data-state="settled">Ready</span>
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
  onRespond,
}: {
  approval: Approval
  busyAction: string | null
  onRespond: (approvalId: string, decision: 'approve' | 'deny') => void
}) {
  return (
    <section aria-labelledby={`approval-${approval.id}`} className="approval-prompt">
      <div className="approval-symbol" aria-hidden="true">
        ?
      </div>
      <div>
        <h3 id={`approval-${approval.id}`}>
          {approval.kind === 'question' ? 'Worker question' : 'Approval required'}
        </h3>
        <p>{approval.summary}</p>
        <div className="approval-actions">
          <button
            className="primary-button"
            disabled={Boolean(busyAction)}
            onClick={() => onRespond(approval.id, 'approve')}
            type="button"
          >
            {busyAction === `${approval.id}:approve` ? 'Allowing…' : 'Allow once'}
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

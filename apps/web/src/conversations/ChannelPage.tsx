import {
  FormEvent,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { useQueryClient } from '@tanstack/react-query'
import {
  Link,
  Navigate,
  useNavigate,
  useParams,
  useSearchParams,
} from 'react-router-dom'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { useAuthenticatedSession } from '../auth/session'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import {
  ProjectPageError,
  ProjectPageLoading,
} from '../projects/ProjectOverviewPage'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { useConversationRealtime } from '../realtime/realtimeContext'
import { AppShell } from '../shell/AppShell'
import {
  archiveConversation,
  createThread,
  getConversationTimeline,
  markConversationSeen,
  searchMentionableActors,
  sendConversationMessage,
} from './api'
import { StructuredComposer } from './StructuredComposer'
import { SystemTimelineEntry } from './SystemTimelineEntry'
import type { ComposerSubmission } from './composerModel'
import {
  useConversationTimeline,
  useProjectConversationSidebar,
} from './hooks'
import { MessageContent } from './MessageContent'
import type {
  ConversationTimelineEntry,
  SidebarThread,
} from './types'

export function ChannelPage() {
  const session = useAuthenticatedSession()
  const { projectId = '', channelId = '' } = useParams()
  const [searchParams] = useSearchParams()
  const includeArchived = searchParams.get('includeArchived') === 'true'
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const project = useProject(projectId)
  const timeline = useConversationTimeline(
    projectId,
    channelId,
    includeArchived,
  )
  const linkedThreadSidebar = useProjectConversationSidebar(
    projectId,
    channelId,
    { expandedChannelId: channelId },
  )
  const realtime = useConversationRealtime(channelId)
  const [olderState, setOlderState] = useState<{
    channelId: string
    entries: ConversationTimelineEntry[]
    nextCursor: string | null
  }>({ channelId: '', entries: [], nextCursor: null })
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [olderError, setOlderError] = useState<string | null>(null)
  const [optimisticKeys, setOptimisticKeys] = useState<string[]>([])
  const markedSeenRef = useRef({ conversationId: '', sequence: 0 })
  const page = timeline.data
  const validBoundary =
    page?.conversation.projectId === projectId &&
    page.conversation.id === channelId &&
    page.conversation.kind === 'channel'

  const olderCursor =
    olderState.channelId === channelId
      ? olderState.nextCursor
      : page?.nextCursor ?? null

  const entries = useMemo(() => {
    const bySequence = new Map<number, ConversationTimelineEntry>()
    const historical =
      olderState.channelId === channelId ? olderState.entries : []
    for (const entry of [
      ...historical,
      ...(validBoundary ? page?.entries ?? [] : []),
    ]) {
      bySequence.set(entry.sequence, entry)
    }
    return [...bySequence.values()].sort(
      (left, right) => left.sequence - right.sequence,
    )
  }, [channelId, olderState, page?.entries, validBoundary])
  const linkedThreadsByMessage = useMemo(() => {
    const linked = new Map<string, SidebarThread[]>()
    const channel = linkedThreadSidebar.data?.channels.find(
      (candidate) => candidate.id === channelId,
    )
    for (const thread of channel?.threads ?? []) {
      if (!thread.originMessageId) continue
      linked.set(thread.originMessageId, [
        ...(linked.get(thread.originMessageId) ?? []),
        thread,
      ])
    }
    return linked
  }, [channelId, linkedThreadSidebar.data])

  useEffect(() => {
    if (
      !validBoundary ||
      !page ||
      !page.capabilities.markSeen ||
      page.highestSequence <= page.readState.highestSeenSequence ||
      (markedSeenRef.current.conversationId === channelId &&
        page.highestSequence <= markedSeenRef.current.sequence) ||
      entries.at(-1)?.sequence !== page.highestSequence
    ) {
      return
    }
    markedSeenRef.current = {
      conversationId: channelId,
      sequence: page.highestSequence,
    }
    void markConversationSeen({
      conversationId: channelId,
      highestSeenSequence: page.highestSequence,
    })
      .then(() =>
        queryClient.invalidateQueries({
          queryKey: queryKeys.conversationSidebarRoot(projectId),
        }),
      )
      .catch(() => {
        markedSeenRef.current = {
          conversationId: channelId,
          sequence: page.readState.highestSeenSequence,
        }
      })
  }, [
    channelId,
    entries,
    page,
    projectId,
    queryClient,
    validBoundary,
  ])

  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'channel', conversationId: channelId },
    project.isSuccess &&
      project.data.capabilities.navigationWrite &&
      timeline.isSuccess &&
      validBoundary &&
      !page?.conversation.archivedAt,
    project.data?.navigation,
  )

  const loadOlder = async () => {
    if (!olderCursor || loadingOlder) return
    setLoadingOlder(true)
    setOlderError(null)
    try {
      const older = await getConversationTimeline(
        channelId,
        olderCursor,
        50,
        includeArchived,
      )
      if (
        older.conversation.id !== channelId ||
        older.conversation.projectId !== projectId ||
        older.conversation.kind !== 'channel'
      ) {
        throw new Error('Older timeline page crossed its Project boundary.')
      }
      setOlderState((current) => ({
        channelId,
        entries: [
          ...older.entries,
          ...(current.channelId === channelId ? current.entries : []),
        ],
        nextCursor: older.nextCursor,
      }))
    } catch (reason) {
      setOlderError(
        reason instanceof Error ? reason.message : 'Older Messages could not load.',
      )
    } finally {
      setLoadingOlder(false)
    }
  }

  const sendMessage = async (submission: ComposerSubmission) => {
    setOptimisticKeys((current) => [...current, submission.idempotencyKey])
    try {
      await sendConversationMessage({
        conversationId: channelId,
        kind: 'channel',
        ...submission,
      })
      await timeline.refetch()
      void queryClient.invalidateQueries({
        queryKey: queryKeys.conversationSidebarRoot(projectId),
      })
    } finally {
      setOptimisticKeys((current) =>
        current.filter((key) => key !== submission.idempotencyKey),
      )
    }
  }

  if (isApiError(timeline.error) && timeline.error.status === 404) {
    return <Navigate replace to={`/projects/${encodeURIComponent(projectId)}`} />
  }
  if (timeline.isSuccess && !validBoundary) {
    return <Navigate replace to={`/projects/${encodeURIComponent(projectId)}`} />
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

  return (
    <AppShell
      mainClassName="workspace conversation-workspace"
      project={project.data}
      selectedChannelId={validBoundary ? channelId : undefined}
      user={session.user}
    >
      <NavigationSyncAlert {...navigationSync} />
      <header className="workspace-heading">
        <div>
          <span className="section-label">Channel</span>
          <h1 id="workspace-heading">
            {validBoundary ? page?.conversation.name : 'Channel'}
          </h1>
        </div>
        {validBoundary && !page?.conversation.archivedAt ? (
          <div className="header-status">
            <span
              className="connection-state"
              data-connection={realtime.connection}
            >
              <span aria-hidden="true">●</span>{' '}
              {realtime.connection === 'live' ? 'Live' : 'Reconnecting'}
            </span>
            {page?.capabilities.archive ? (
              <button
                className="secondary-button"
                onClick={() => {
                  if (!page) return
                  void archiveConversation({
                    conversationId: channelId,
                    kind: 'channel',
                    expectedVersion: page.conversation.version,
                  }).then(() =>
                    navigate(`/projects/${encodeURIComponent(projectId)}`, {
                      replace: true,
                    }),
                  )
                }}
                type="button"
              >
                Archive Channel
              </button>
            ) : null}
          </div>
        ) : null}
      </header>

      <section
        aria-busy={timeline.isPending}
        aria-label="Channel timeline"
        className="timeline conversation-timeline"
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
        {olderError ? <p role="alert">{olderError}</p> : null}
        {timeline.isPending ? <p role="status">Loading Channel…</p> : null}
        {timeline.isError ? (
          <div className="error-state" role="alert">
            <h2>Channel unavailable</h2>
            <p>The Channel does not exist or is no longer available to you.</p>
            <button onClick={() => void timeline.refetch()} type="button">
              Try again
            </button>
          </div>
        ) : null}
        {validBoundary && entries.length === 0 ? (
          <div className="empty-state">
            <h2>Start this Channel.</h2>
            <p>Messages remain ordered and durable across sessions.</p>
          </div>
        ) : null}
        {entries.map((entry) => (
          <ConversationEntry
            canCreateThread={Boolean(
              page?.capabilities.sendMessage &&
                !page.conversation.archivedAt,
            )}
            channelId={channelId}
            entry={entry}
            key={entry.id}
            linkedThreads={
              entry.message
                ? linkedThreadsByMessage.get(entry.message.id) ?? []
                : []
            }
            onCreated={(threadId) =>
              navigate(
                `/projects/${encodeURIComponent(projectId)}/threads/${encodeURIComponent(threadId)}`,
              )
            }
            projectId={projectId}
          />
        ))}
        {optimisticKeys.map((key) => (
          <article aria-label="Message sending" className="message pending-message" key={key}>
            <header>
              <strong>@{session.user.loginName}</strong>
              <span>Sending…</span>
            </header>
          </article>
        ))}
        {validBoundary && !page?.conversation.archivedAt ? (
          <ThreadCreator
            channelId={channelId}
            label="Start a Thread"
            onCreated={(threadId) =>
              navigate(
                `/projects/${encodeURIComponent(projectId)}/threads/${encodeURIComponent(threadId)}`,
              )
            }
          />
        ) : null}
      </section>

      {validBoundary && page?.capabilities.sendMessage && !page.conversation.archivedAt ? (
        <StructuredComposer
          onSubmit={sendMessage}
          searchActors={async (query) =>
            (await searchMentionableActors(projectId, query)).actors
          }
        />
      ) : null}
    </AppShell>
  )
}

function ConversationEntry({
  canCreateThread,
  channelId,
  entry,
  linkedThreads,
  onCreated,
  projectId,
}: {
  canCreateThread: boolean
  channelId: string
  entry: ConversationTimelineEntry
  linkedThreads: SidebarThread[]
  onCreated: (threadId: string) => void
  projectId: string
}) {
  if (entry.message) {
    const worker = entry.message.author.kind === 'execution_worker'
    return (
      <article className={`message ${worker ? 'worker-message' : ''}`}>
        <header>
          <span className="avatar" aria-hidden="true">
            {worker ? 'A' : entry.message.author.displayName.charAt(0)}
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
            actors={entry.message.mentionedActors}
            content={entry.message.content}
            fallbackPlainText={entry.message.plainText}
            workItems={entry.message.referencedWorkItems}
          />
        </p>
        <div className="message-thread-actions">
          {linkedThreads.map((thread) => (
            <Link
              key={thread.id}
              to={`/projects/${encodeURIComponent(projectId)}/threads/${encodeURIComponent(thread.id)}`}
            >
              Open linked Thread: {thread.name}
            </Link>
          ))}
          {linkedThreads.length === 0 && canCreateThread ? (
            <ThreadCreator
              channelId={channelId}
              label="Start a linked Thread"
              onCreated={onCreated}
              originMessageId={entry.message.id}
            />
          ) : null}
        </div>
      </article>
    )
  }
  if (entry.workerEvent) {
    return (
      <article className="provider-event">
        <span aria-hidden="true">·</span>
        <div>
          <strong>{entry.workerEvent.kind.replaceAll('_', ' ')}</strong>
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

function ThreadCreator({
  channelId,
  label,
  onCreated,
  originMessageId,
}: {
  channelId: string
  label: string
  onCreated: (threadId: string) => void
  originMessageId?: string
}) {
  const keyRef = useRef(crypto.randomUUID())
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const title = name.trim()
    if (!title || busy) return
    setBusy(true)
    setError(null)
    void createThread({
      channelId,
      name: title,
      originMessageId,
      idempotencyKey: keyRef.current,
    })
      .then((thread) => {
        keyRef.current = crypto.randomUUID()
        setName('')
        onCreated(thread.id)
      })
      .catch((reason: unknown) => {
        setError(reason instanceof Error ? reason.message : 'Thread could not be created.')
      })
      .finally(() => setBusy(false))
  }
  return (
    <details className="thread-creator">
      <summary>{label}</summary>
      <form onSubmit={submit}>
        <label htmlFor={`thread-name-${originMessageId ?? 'channel'}`}>
          Thread title
        </label>
        <input
          id={`thread-name-${originMessageId ?? 'channel'}`}
          maxLength={120}
          onChange={(event) => setName(event.target.value)}
          value={name}
        />
        <button disabled={!name.trim() || busy} type="submit">
          {busy ? 'Creating…' : 'Create Thread'}
        </button>
        {error ? <p role="alert">{error}</p> : null}
      </form>
    </details>
  )
}

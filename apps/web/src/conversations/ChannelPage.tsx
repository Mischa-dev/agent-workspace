import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, Navigate, useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { useAuthenticatedSession } from '../auth/session'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { Banner, Button, ConversationIntro, Dialog, ErrorState, Icon, IconButton, MessageSkeleton } from '../ui'
import { searchWorkItemReferences } from '../work-items/api'
import {
  archiveConversation,
  getConversationTimeline,
  markConversationSeen,
  searchMentionableActors,
  sendConversationMessage,
} from './api'
import { ConversationHeader } from './ConversationHeader'
import type { ComposerSubmission } from './composerModel'
import { useConversationTimeline, useProjectConversationSidebar } from './hooks'
import { StructuredComposer } from './StructuredComposer'
import { ThreadDialog } from './ThreadDialog'
import { ConversationTimeline, OlderMessagesButton, PendingMessageRow, TimelineEntries } from './Timeline'
import type { ConversationTimelineEntry, SidebarThread } from './types'

type ThreadOrigin = { messageId: string; text: string; author: string }

const THREAD_STATE: Record<SidebarThread['attention'], string> = {
  response_required: 'Needs you',
  active: 'Working',
  failed: 'Run failed',
  completed: 'Done',
  unread: 'New replies',
  settled: '',
}

export function ChannelPage() {
  const session = useAuthenticatedSession()
  const { projectId = '', channelId = '' } = useParams()
  const [searchParams] = useSearchParams()
  const includeArchived = searchParams.get('includeArchived') === 'true'
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const project = useProject(projectId)
  const timeline = useConversationTimeline(projectId, channelId, includeArchived)
  const linkedThreadSidebar = useProjectConversationSidebar(projectId, channelId, { expandedChannelId: channelId })
  const [olderState, setOlderState] = useState<{
    channelId: string
    entries: ConversationTimelineEntry[]
    nextCursor: string | null
  }>({ channelId: '', entries: [], nextCursor: null })
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [olderError, setOlderError] = useState<string | null>(null)
  const [pending, setPending] = useState<{ key: string; text?: string }[]>([])
  const [threadDialog, setThreadDialog] = useState<{ origin: ThreadOrigin | null } | null>(null)
  const [archiveOpen, setArchiveOpen] = useState(false)
  const [archiveError, setArchiveError] = useState<string | null>(null)
  const markedSeenRef = useRef({ conversationId: '', sequence: 0 })
  const [firstUnread, setFirstUnread] = useState<{ channelId: string; sequence: number | null }>({ channelId: '', sequence: null })
  const page = timeline.data
  const validBoundary =
    page?.conversation.projectId === projectId && page.conversation.id === channelId && page.conversation.kind === 'channel'

  // Remember where unread started when the channel opened, so the New line doesn't jump as it's marked seen.
  if (validBoundary && page && firstUnread.channelId !== channelId) {
    setFirstUnread({
      channelId,
      sequence: page.highestSequence > page.readState.highestSeenSequence ? page.readState.highestSeenSequence + 1 : null,
    })
  }

  const olderCursor = olderState.channelId === channelId ? olderState.nextCursor : page?.nextCursor ?? null

  const entries = useMemo(() => {
    const bySequence = new Map<number, ConversationTimelineEntry>()
    const historical = olderState.channelId === channelId ? olderState.entries : []
    for (const entry of [...historical, ...(validBoundary ? page?.entries ?? [] : [])]) {
      bySequence.set(entry.sequence, entry)
    }
    return [...bySequence.values()].sort((left, right) => left.sequence - right.sequence)
  }, [channelId, olderState, page?.entries, validBoundary])

  const linkedThreadsByMessage = useMemo(() => {
    const linked = new Map<string, SidebarThread[]>()
    const channel = linkedThreadSidebar.data?.channels.find((candidate) => candidate.id === channelId)
    for (const thread of channel?.threads ?? []) {
      if (!thread.originMessageId) continue
      linked.set(thread.originMessageId, [...(linked.get(thread.originMessageId) ?? []), thread])
    }
    return linked
  }, [channelId, linkedThreadSidebar.data])

  useEffect(() => {
    if (
      !validBoundary ||
      !page ||
      !page.capabilities.markSeen ||
      page.highestSequence <= page.readState.highestSeenSequence ||
      (markedSeenRef.current.conversationId === channelId && page.highestSequence <= markedSeenRef.current.sequence) ||
      entries.at(-1)?.sequence !== page.highestSequence
    ) {
      return
    }
    markedSeenRef.current = { conversationId: channelId, sequence: page.highestSequence }
    void markConversationSeen({ conversationId: channelId, highestSeenSequence: page.highestSequence })
      .then(() => queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) }))
      .catch(() => {
        markedSeenRef.current = { conversationId: channelId, sequence: page.readState.highestSeenSequence }
      })
  }, [channelId, entries, page, projectId, queryClient, validBoundary])

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
      const older = await getConversationTimeline(channelId, olderCursor, 50, includeArchived)
      if (older.conversation.id !== channelId || older.conversation.projectId !== projectId || older.conversation.kind !== 'channel') {
        throw new Error('Older timeline page crossed its Project boundary.')
      }
      setOlderState((current) => ({
        channelId,
        entries: [...older.entries, ...(current.channelId === channelId ? current.entries : [])],
        nextCursor: older.nextCursor,
      }))
    } catch (reason) {
      setOlderError(reason instanceof Error ? reason.message : 'Earlier messages didn’t load.')
    } finally {
      setLoadingOlder(false)
    }
  }

  const sendMessage = async (submission: ComposerSubmission) => {
    setPending((current) => [...current, { key: submission.idempotencyKey, text: submission.text }])
    try {
      await sendConversationMessage({ conversationId: channelId, kind: 'channel', ...submission })
      await timeline.refetch()
      void queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) })
    } finally {
      setPending((current) => current.filter((item) => item.key !== submission.idempotencyKey))
    }
  }

  const archive = () => {
    if (!page) return
    setArchiveError(null)
    void archiveConversation({ conversationId: channelId, kind: 'channel', expectedVersion: page.conversation.version })
      .then(() => {
        setArchiveOpen(false)
        void queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) })
        navigate(`/projects/${encodeURIComponent(projectId)}`, { replace: true })
      })
      .catch((reason: unknown) => setArchiveError(reason instanceof Error ? reason.message : 'The channel wasn’t archived.'))
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

  const name = validBoundary ? page?.conversation.name ?? 'Channel' : 'Channel'
  const archived = Boolean(page?.conversation.archivedAt)
  const canPost = Boolean(validBoundary && page?.capabilities.sendMessage && !archived)
  const threadRoute = (threadId: string) => `/projects/${encodeURIComponent(projectId)}/threads/${encodeURIComponent(threadId)}`

  return (
    <AppShell
      mainClassName="aw-main--conversation"
      project={project.data}
      selectedChannelId={validBoundary ? channelId : undefined}
      user={session.user}
    >
      <NavigationSyncAlert {...navigationSync} />
      <ConversationHeader
        actions={
          canPost ? <IconButton icon="messages" label="Start a thread" onClick={() => setThreadDialog({ origin: null })} side="bottom" /> : null
        }
        icon="hash"
        menu={[
          ...(canPost ? [{ label: 'Start a thread', icon: 'messages' as const, onSelect: () => setThreadDialog({ origin: null }) }] : []),
          {
            label: 'Archived conversations',
            icon: 'archive' as const,
            onSelect: () => navigate(`/projects/${encodeURIComponent(projectId)}/conversations/archived`),
          },
          ...(page?.capabilities.archive && !archived
            ? [{ separator: true as const }, { label: 'Archive channel', icon: 'archive' as const, danger: true, onSelect: () => setArchiveOpen(true) }]
            : []),
        ]}
        title={name}
      />
      {archived ? (
        <Banner className="aw-conv__banner" icon="archive" title="This channel is archived." tone="offline">
          You can read it, but no one can post.
        </Banner>
      ) : null}

      <ConversationTimeline busy={timeline.isPending} label={`Messages in ${name}`} revision={`${page?.highestSequence ?? 0}:${pending.length}:${olderState.entries.length}`}>
        {olderCursor ? <OlderMessagesButton busy={loadingOlder} onClick={() => void loadOlder()} /> : null}
        {olderError ? <Banner tone="danger">{olderError}</Banner> : null}
        {timeline.isPending ? <MessageSkeleton /> : null}
        {timeline.isError ? (
          <ErrorState action={<Button onClick={() => void timeline.refetch()}>Try again</Button>} title="This channel didn’t load">
            It may have been removed, or you may no longer have access.
          </ErrorState>
        ) : null}
        {validBoundary && !olderCursor ? (
          <ConversationIntro
            actions={
              canPost ? (
                <Button icon="messages" onClick={() => setThreadDialog({ origin: null })} size="sm">
                  Start a thread
                </Button>
              ) : undefined
            }
            title={
              <>
                This is the very beginning of <span className="aw-intro__name">#{name}</span>
              </>
            }
          >
            {archived
              ? 'This channel is archived. Its history stays here to read.'
              : `Everyone in ${project.data.name} can read and post here. Mention someone with @ to ask them directly.`}
          </ConversationIntro>
        ) : null}
        <TimelineEntries
          entries={entries}
          firstUnreadSequence={firstUnread.channelId === channelId ? firstUnread.sequence : null}
          renderExtras={(entry) => {
            const threads = entry.message ? linkedThreadsByMessage.get(entry.message.id) ?? [] : []
            return threads.map((thread) => (
              <Link className="aw-threadlink" key={thread.id} to={threadRoute(thread.id)}>
                <Icon name="messages" size={14} />
                <span className="aw-threadlink__name">{thread.name}</span>
                {THREAD_STATE[thread.attention] ? <span className="aw-threadlink__state">{THREAD_STATE[thread.attention]}</span> : null}
                <Icon className="aw-threadlink__chev" name="chevron-right" size={14} />
                <span className="visually-hidden">, open thread</span>
              </Link>
            ))
          }}
          renderToolbar={(entry) => {
            if (!entry.message) return null
            const threads = linkedThreadsByMessage.get(entry.message.id) ?? []
            if (threads.length) {
              return <IconButton icon="messages" label="Open thread" onClick={() => navigate(threadRoute(threads[0].id))} size="sm" />
            }
            if (!canPost) return null
            const message = entry.message
            return (
              <IconButton
                icon="messages"
                label="Start a thread"
                onClick={() =>
                  setThreadDialog({ origin: { messageId: message.id, text: message.plainText, author: message.author.displayName } })
                }
                size="sm"
              />
            )
          }}
          viewerId={session.user.actorId}
        />
        {pending.map((item, index) => (
          <PendingMessageRow
            continued={index > 0 || entries.at(-1)?.message?.author.id === session.user.actorId}
            key={item.key}
            name={session.user.displayName}
            seed={session.user.actorId}
            text={item.text}
          />
        ))}
      </ConversationTimeline>

      {canPost ? (
        <StructuredComposer
          label={`Message #${name}`}
          onSubmit={sendMessage}
          placeholder={`Message #${name}`}
          searchActors={async (query) => (await searchMentionableActors(projectId, query)).actors}
          searchWorkItems={(query) => searchWorkItemReferences(projectId, query)}
        />
      ) : validBoundary && archived ? (
        <StructuredComposer
          disabledReason="This channel is archived. Nobody can post here."
          onSubmit={async () => undefined}
          searchActors={async () => []}
        />
      ) : null}

      {threadDialog ? (
        <ThreadDialog
          channelId={channelId}
          onClose={() => setThreadDialog(null)}
          onCreated={(threadId) => {
            void queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) })
            navigate(threadRoute(threadId))
          }}
          open
          origin={threadDialog.origin}
        />
      ) : null}
      <Dialog
        description="People can still read it from Archived conversations, but no one can post."
        footer={
          <>
            <Button onClick={() => setArchiveOpen(false)}>Cancel</Button>
            <Button onClick={archive} variant="danger">
              Archive channel
            </Button>
          </>
        }
        onClose={() => setArchiveOpen(false)}
        open={archiveOpen}
        title={`Archive #${name}?`}
      >
        {archiveError ? <Banner tone="danger">{archiveError}</Banner> : null}
      </Dialog>
    </AppShell>
  )
}

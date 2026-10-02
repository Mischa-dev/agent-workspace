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
import { MessageMemoryActions, PinnedBar } from '../memory/ConversationMemory'
import { useMessageActions } from './MessageActions'
import { StructuredComposer } from './StructuredComposer'
import { ThreadBoard, ThreadStatusChip } from './threadBoard'
import { useThreads } from './threadModel'
import { ThreadDialog } from './ThreadDialog'
import { ConversationTimeline, OlderMessagesButton, PendingMessageRow, TimelineEntries } from './Timeline'
import type { ConversationTimelineEntry, SidebarThread } from './types'
import { useLiveOlderEntries } from './useLiveOlderEntries'

const NO_ENTRIES: ConversationTimelineEntry[] = []

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
  const [searchParams, setSearchParams] = useSearchParams()
  const includeArchived = searchParams.get('includeArchived') === 'true'
  const boardOpen = searchParams.get('threads') === 'all'
  const setBoardOpen = (open: boolean) => {
    const next = new URLSearchParams(searchParams)
    if (open) next.set('threads', 'all')
    else next.delete('threads')
    setSearchParams(next, { replace: true })
  }
  const threadStatuses = useThreads(channelId)
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const project = useProject(projectId)
  const timeline = useConversationTimeline(projectId, channelId, includeArchived)
  const linkedThreadSidebar = useProjectConversationSidebar(projectId, channelId, { expandedChannelId: channelId })
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

  const older = useLiveOlderEntries({
    conversationId: channelId,
    newestEntries: validBoundary && page ? page.entries : NO_ENTRIES,
    newestCursor: validBoundary ? page?.nextCursor ?? null : null,
    loadPage: async (cursor) => {
      const result = await getConversationTimeline(channelId, cursor, 50, includeArchived)
      if (result.conversation.id !== channelId || result.conversation.projectId !== projectId || result.conversation.kind !== 'channel') {
        throw new Error('Older timeline page crossed its Project boundary.')
      }
      return result
    },
  })
  const entries = older.entries
  const olderCursor = older.cursor

  const statusByThread = useMemo(
    () => new Map((threadStatuses.data ?? []).map((thread) => [thread.id, thread])),
    [threadStatuses.data],
  )

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

  const name = validBoundary ? page?.conversation.name ?? 'Channel' : 'Channel'
  const archived = Boolean(page?.conversation.archivedAt)
  const canPost = Boolean(validBoundary && page?.capabilities.sendMessage && !archived)
  const threadRoute = (threadId: string) => `/projects/${encodeURIComponent(projectId)}/threads/${encodeURIComponent(threadId)}`
  const searchActors = async (query: string) => (await searchMentionableActors(projectId, query)).actors
  const searchWorkItems = (query: string) => searchWorkItemReferences(projectId, query)

  const messageActions = useMessageActions({
    projectId,
    conversationId: channelId,
    viewerId: session.user.actorId,
    capabilities: validBoundary ? page?.capabilities : null,
    searchActors,
    searchWorkItems,
    onReactionsChanged: older.patchReactions,
    onChanged: (entry) => {
      older.replaceEntry(entry)
      void timeline.refetch()
    },
    actionsFor: (message) => {
      const threads = linkedThreadsByMessage.get(message.id) ?? []
      const memory = canPost ? <MessageMemoryActions conversationId={channelId} message={message} projectId={projectId} /> : null
      if (threads.length) {
        return (
          <>
            <IconButton icon="messages" label="Open thread" onClick={() => navigate(threadRoute(threads[0].id))} size="sm" />
            {memory}
          </>
        )
      }
      if (!canPost) return null
      return (
        <>
          <IconButton
            icon="messages"
            label="Start a thread"
            onClick={() => setThreadDialog({ origin: { messageId: message.id, text: message.plainText, author: message.author.displayName } })}
            size="sm"
          />
          {memory}
        </>
      )
    },
  })

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

  return (
    <AppShell
      context={
        boardOpen ? (
          <ThreadBoard
            conversationId={channelId}
            onClose={() => setBoardOpen(false)}
            onOpen={(thread) => navigate(threadRoute(thread.id))}
          />
        ) : undefined
      }
      contextKey={boardOpen ? 'threads' : undefined}
      mainClassName="aw-main--conversation"
      onContextDismiss={() => setBoardOpen(false)}
      project={project.data}
      selectedChannelId={validBoundary ? channelId : undefined}
      user={session.user}
    >
      <NavigationSyncAlert {...navigationSync} />
      <ConversationHeader
        actions={
          <>
            <IconButton
              icon="list"
              label={boardOpen ? 'Hide threads' : 'Threads'}
              onClick={() => setBoardOpen(!boardOpen)}
              pressed={boardOpen}
              side="bottom"
            />
            {canPost ? <IconButton icon="messages" label="Start a thread" onClick={() => setThreadDialog({ origin: null })} side="bottom" /> : null}
          </>
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
      {validBoundary ? <PinnedBar canEdit={canPost} conversationId={channelId} projectId={projectId} /> : null}
      {archived ? (
        <Banner className="aw-conv__banner" icon="archive" title="This channel is archived." tone="offline">
          You can read it, but no one can post.
        </Banner>
      ) : null}

      <ConversationTimeline busy={timeline.isPending} label={`Messages in ${name}`} revision={`${page?.highestSequence ?? 0}:${pending.length}:${older.olderCount}`}>
        {olderCursor ? <OlderMessagesButton busy={older.loading} onClick={() => void older.loadOlder()} /> : null}
        {older.error ? <Banner tone="danger">{older.error}</Banner> : null}
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
                {statusByThread.get(thread.id) ? <ThreadStatusChip status={statusByThread.get(thread.id)?.status ?? 'Open'} /> : null}
                {THREAD_STATE[thread.attention] ? <span className="aw-threadlink__state">{THREAD_STATE[thread.attention]}</span> : null}
                {statusByThread.get(thread.id)?.outcome ? (
                  <span className="aw-threadlink__state">{statusByThread.get(thread.id)?.outcome}</span>
                ) : null}
                <Icon className="aw-threadlink__chev" name="chevron-right" size={14} />
                <span className="visually-hidden">, open thread</span>
              </Link>
            ))
          }}
          onOpenHistory={messageActions.openHistory}
          renderEditor={messageActions.editorFor}
          renderReactions={messageActions.reactionsFor}
          renderToolbar={messageActions.toolbarFor}
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
          searchActors={searchActors}
          searchWorkItems={searchWorkItems}
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
      {messageActions.dialogs}
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

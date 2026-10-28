import { useCallback, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Navigate, useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { useAuthenticatedSession } from '../auth/session'
import { useProjectExperience } from '../experience/layout'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { Banner, Button, ConversationIntro, ErrorState, IconButton, MessageSkeleton } from '../ui'
import { searchWorkItemReferences } from '../work-items/api'
import { getConversationTimeline, retryReplyRequest, searchMentionableActors, stopReplyRequest } from './api'
import { useArchiveConversation, useRestoreConversation } from './useArchiveConversation'
import { useAutoMarkSeen } from './useAutoMarkSeen'
import { ConversationHeader } from './ConversationHeader'
import { useProjectConversationSidebar } from './hooks'
import { useConversationTimeline } from './useConversationTimeline'
import { ReplyStatusList } from './ReplyStatus'
import { groupReplyRequests } from './replyStatusModel'
import { PinnedBar } from '../memory/ConversationMemory'
import { useConversationTools } from '../later/useConversationTools'
import { useMessageActions } from './MessageActions'
import { StructuredComposer } from './StructuredComposer'
import { ThreadBoard } from './threadBoard'
import { ThreadChip } from './ThreadChip'
import { threadChipFacts } from './threadChipModel'
import { useThreads } from './threadModel'
import { ThreadDialog } from './ThreadDialog'
import { ThreadPanel } from '../threads/ThreadPanel'
import { usePageTitle } from '../shell/documentTitleModel'
import { uploadFile } from '../files/api'
import { ConversationTimeline, OlderMessagesButton, PendingMessages, TimelineEntries } from './Timeline'
import type { ConversationTimelineEntry, SidebarThread } from './types'
import { useFirstUnread } from './useFirstUnread'
import { useLiveOlderEntries } from './useLiveOlderEntries'
import { useMessageSender } from './useMessageSender'

const NO_ENTRIES: ConversationTimelineEntry[] = []

type ThreadOrigin = { messageId: string; text: string; author: string }

export function ChannelPage() {
  const session = useAuthenticatedSession()
  const viewer = { actorId: session.user.actorId, displayName: session.user.displayName }
  const { projectId = '', channelId = '' } = useParams()
  const [searchParams, setSearchParams] = useSearchParams()
  const includeArchived = searchParams.get('includeArchived') === 'true'
  const boardOpen = searchParams.get('threads') === 'all'
  const openThreadId = searchParams.get('thread')
  const setBoardOpen = (open: boolean) => {
    const next = new URLSearchParams(searchParams)
    // The board and an open thread share the side panel: showing one closes the other.
    if (open) {
      next.set('threads', 'all')
      next.delete('thread')
    } else next.delete('threads')
    setSearchParams(next, { replace: true })
  }
  // Threads open beside the channel, so you keep your place in it.
  const openThread = (threadId: string | null) => {
    const next = new URLSearchParams(searchParams)
    next.delete('threads')
    if (threadId) next.set('thread', threadId)
    else next.delete('thread')
    setSearchParams(next)
  }
  const threadStatuses = useThreads(projectId, channelId)
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const project = useProject(projectId)
  const timeline = useConversationTimeline(projectId, channelId, includeArchived)
  const linkedThreadSidebar = useProjectConversationSidebar(projectId, channelId, { expandedChannelId: channelId })
  // Kept per channel: the page stays mounted from one channel to the next.
  const [threadDialog, setThreadDialog] = useState<{ channelId: string; origin: ThreadOrigin | null } | null>(null)
  const startThread = (origin: ThreadOrigin | null) => setThreadDialog({ channelId, origin })
  const page = timeline.data
  const requestsByMessage = useMemo(() => groupReplyRequests(page?.replyRequests ?? []), [page?.replyRequests])
  const validBoundary =
    page?.conversation.projectId === projectId && page.conversation.id === channelId && page.conversation.kind === 'channel'

  const older = useLiveOlderEntries({
    conversationId: channelId,
    newestEntries: validBoundary && page ? page.entries : NO_ENTRIES,
    newestCursor: validBoundary ? (page?.nextCursor ?? null) : null,
    loadPage: async (cursor) => {
      const result = await getConversationTimeline(channelId, cursor, 50, includeArchived)
      if (result.conversation.id !== channelId || result.conversation.projectId !== projectId || result.conversation.kind !== 'channel') {
        throw new Error('Older timeline page crossed its project boundary.')
      }
      return result
    },
  })
  const entries = older.entries
  const olderCursor = older.cursor
  const loadOlder = older.loadOlder

  const unread = useFirstUnread({
    conversationId: channelId,
    entries,
    fresh: timeline.isFetchedAfterMount,
    hasOlder: Boolean(olderCursor),
    page: validBoundary ? page : null,
    viewerId: viewer.actorId,
  })

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

  const refreshSidebar = useCallback(
    () => queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) }),
    [projectId, queryClient],
  )
  useAutoMarkSeen(
    {
      conversationId: channelId,
      highestSequence: page?.highestSequence ?? 0,
      highestSeenSequence: page?.readState.highestSeenSequence ?? 0,
      // Only once the newest entry is on screen.
      ready: Boolean(validBoundary && page?.capabilities.markSeen && entries.at(-1)?.sequence === page.highestSequence),
    },
    refreshSidebar,
  )

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

  const name = validBoundary ? (page?.conversation.name ?? 'Channel') : 'Channel'
  usePageTitle(validBoundary ? `#${name}` : null)
  const archived = Boolean(page?.conversation.archivedAt)
  const canPost = Boolean(validBoundary && page?.capabilities.sendMessage && !archived)
  // A layout without threads keeps the threads already here reachable but doesn't offer new ones.
  const threadsShown = useProjectExperience(projectId).shows('talk.threads')
  const canStartThread = canPost && threadsShown
  const threadRoute = (threadId: string) => `/projects/${encodeURIComponent(projectId)}/threads/${encodeURIComponent(threadId)}`
  const searchActors = async (query: string) => (await searchMentionableActors(projectId, query)).actors
  const searchWorkItems = (query: string) => searchWorkItemReferences(projectId, query)

  const sender = useMessageSender({
    conversationId: channelId,
    kind: 'channel',
    onSent: () => void refreshSidebar(),
    queryKey: queryKeys.timeline(projectId, channelId, includeArchived),
  })

  const messageActions = useMessageActions({
    projectId,
    conversationId: channelId,
    viewerId: viewer.actorId,
    capabilities: validBoundary ? page?.capabilities : null,
    searchActors,
    searchWorkItems,
    taskProject: true,
    onReactionsChanged: older.patchReactions,
    onChanged: (entry) => {
      older.replaceEntry(entry)
      void timeline.refetch()
    },
    onMarkedUnread: (entry) => unread.markUnreadFrom(entry.sequence),
    primaryFor: (message) => {
      const threads = linkedThreadsByMessage.get(message.id) ?? []
      if (threads.length) {
        return <IconButton icon="messages" label="Open thread" onClick={() => openThread(threads[0].id)} size="sm" />
      }
      if (!canStartThread) return null
      return (
        <IconButton
          icon="messages"
          label="Start a thread"
          onClick={() => startThread({ messageId: message.id, text: message.plainText, author: message.author.displayName })}
          size="sm"
        />
      )
    },
  })

  const tools = useConversationTools({
    canPost,
    conversationId: channelId,
    kind: 'channel',
    latestMessageId: entries.at(-1)?.message?.id,
    projectId,
  })

  const archiveChannel = useArchiveConversation({
    conversationId: channelId,
    kind: 'channel',
    label: `#${name}`,
    leaveTo: `/projects/${encodeURIComponent(projectId)}`,
    projectId,
  })
  const restoreChannel = useRestoreConversation({ conversationId: channelId, kind: 'channel', label: `#${name}`, projectId })
  const canArchive = Boolean(validBoundary && page?.capabilities.archive)
  const canRestore = Boolean(validBoundary && page?.capabilities.restore)
  const restore = () => {
    if (page && !restoreChannel.isPending) restoreChannel.mutate(page.conversation.version)
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
        <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  const dialog = threadDialog?.channelId === channelId ? threadDialog : null

  return (
    <AppShell
      context={
        openThreadId && validBoundary ? (
          <ThreadPanel
            fullPageHref={threadRoute(openThreadId)}
            key={openThreadId}
            onClose={() => openThread(null)}
            parentId={channelId}
            parentName={`#${name}`}
            projectId={projectId}
            searchActors={searchActors}
            searchWorkItems={searchWorkItems}
            threadId={openThreadId}
            viewer={viewer}
          />
        ) : boardOpen ? (
          <ThreadBoard
            conversationId={channelId}
            onClose={() => setBoardOpen(false)}
            onOpen={(thread) => openThread(thread.id)}
            onStart={canStartThread ? () => startThread(null) : undefined}
            projectId={projectId}
          />
        ) : undefined
      }
      contextKey={openThreadId ?? (boardOpen ? 'threads' : undefined)}
      mainClassName="aw-main--conversation"
      onContextDismiss={() => (openThreadId ? openThread(null) : setBoardOpen(false))}
      project={project.data}
      selectedChannelId={validBoundary ? channelId : undefined}
      user={session.user}
    >
      <NavigationSyncAlert {...navigationSync} />
      <ConversationHeader
        actions={
          <>
            {threadsShown || boardOpen ? (
              <IconButton
                icon="list"
                label={boardOpen ? 'Hide threads' : 'Threads'}
                onClick={() => setBoardOpen(!boardOpen)}
                pressed={boardOpen}
                side="bottom"
              />
            ) : null}
            {canStartThread ? <IconButton icon="messages" label="Start a thread" onClick={() => startThread(null)} side="bottom" /> : null}
          </>
        }
        icon="hash"
        menu={[
          ...(canStartThread ? [{ label: 'Start a thread', icon: 'messages' as const, onSelect: () => startThread(null) }] : []),
          ...tools.levelMenu,
          { separator: true as const },
          {
            label: 'Archived conversations',
            icon: 'archive' as const,
            onSelect: () => navigate(`/projects/${encodeURIComponent(projectId)}/conversations/archived`),
          },
          ...(canRestore ? [{ label: 'Restore channel', icon: 'undo' as const, onSelect: restore }] : []),
          ...(canArchive && !archived && page
            ? [
                { separator: true as const },
                // Archiving can be undone (from the toast, or Restore later), so it happens at once.
                { label: 'Archive channel', icon: 'archive' as const, danger: true, onSelect: () => archiveChannel.mutate(page.conversation.version) },
              ]
            : []),
        ]}
        tag={tools.levelMark}
        title={name}
      />
      {tools.banner}
      {validBoundary ? <PinnedBar canEdit={canPost} conversationId={channelId} projectId={projectId} /> : null}

      <ConversationTimeline
        busy={timeline.isPending || older.loading}
        hasOlder={Boolean(olderCursor)}
        key={channelId}
        label={`Messages in ${name}`}
        olderKey={older.olderCount}
        onLoadOlder={() => void loadOlder()}
        revision={`${page?.highestSequence ?? 0}:${sender.pending.length}`}
        stickKey={sender.sent}
        unread={unread.unread}
      >
        {olderCursor ? <OlderMessagesButton busy={older.loading} onClick={() => void loadOlder()} /> : null}
        {older.error ? (
          <Banner onRetry={() => void loadOlder()} title="Earlier messages didn’t load." tone="danger">
            {older.error}
          </Banner>
        ) : null}
        {timeline.isPending ? <MessageSkeleton /> : null}
        {timeline.isError ? <ErrorState error={timeline.error} onRetry={() => void timeline.refetch()} title="This channel didn’t load" /> : null}
        {validBoundary && !olderCursor ? (
          <ConversationIntro
            actions={
              canStartThread ? (
                <Button icon="messages" onClick={() => startThread(null)} size="sm">
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
            {archived ? null : `Everyone in ${project.data.name} can read and post here. Mention someone with @ to ask them directly.`}
          </ConversationIntro>
        ) : null}
        <TimelineEntries
          conversationId={channelId}
          entries={entries}
          firstUnreadSequence={unread.firstUnreadSequence}
          renderExtras={(entry) => {
            const threads = entry.message ? (linkedThreadsByMessage.get(entry.message.id) ?? []) : []
            const requests = entry.message && !entry.message.removed ? (requestsByMessage.get(entry.message.id) ?? []) : []
            return [
              requests.length ? (
                <ReplyStatusList
                  key="replies"
                  onRetry={(requestId) => retryReplyRequest(requestId).then(() => timeline.refetch())}
                  onStop={page?.capabilities.sendMessage ? (requestId) => stopReplyRequest(requestId).then(() => timeline.refetch()) : undefined}
                  requests={requests}
                />
              ) : null,
              ...threads.map((thread) => {
                const facts = threadChipFacts(statusByThread.get(thread.id), thread)
                return facts ? (
                  <ThreadChip
                    current={thread.id === openThreadId}
                    href={threadRoute(thread.id)}
                    key={thread.id}
                    onOpen={() => openThread(thread.id)}
                    thread={facts}
                  />
                ) : null
              }),
            ]
          }}
          onOpenHistory={messageActions.openHistory}
          renderEditor={messageActions.editorFor}
          renderReactions={messageActions.reactionsFor}
          renderToolbar={messageActions.toolbarFor}
          viewerId={viewer.actorId}
        />
        <PendingMessages entries={entries} items={sender.pending} viewer={viewer} />
      </ConversationTimeline>

      {canPost ? (
        <StructuredComposer
          autoFocus={searchParams.get('compose') === '1'}
          draftKey={channelId}
          focusOnOpen={!openThreadId}
          label={`Message #${name}`}
          outbox={{
            kind: 'message',
            conversationId: channelId,
            conversationKind: 'channel',
            href: `/projects/${encodeURIComponent(projectId)}/channels/${encodeURIComponent(channelId)}`,
          }}
          onEditLast={() => messageActions.editLast(entries)}
          onSubmit={sender.send}
          placeholder={`Message #${name}`}
          searchActors={searchActors}
          searchWorkItems={searchWorkItems}
          upload={uploadFile({ kind: 'conversation', conversationId: channelId })}
          {...tools.composer}
        />
      ) : validBoundary && archived ? (
        <StructuredComposer
          disabledReason={
            <>
              This channel is archived. You can read it, but no one can post.
              {canRestore ? (
                <Button busy={restoreChannel.isPending} icon="undo" onClick={restore} size="sm">
                  Restore channel
                </Button>
              ) : null}
            </>
          }
          onSubmit={async () => undefined}
          searchActors={async () => []}
        />
      ) : null}

      {dialog ? (
        <ThreadDialog
          channelId={channelId}
          onClose={() => setThreadDialog(null)}
          onCreated={(threadId) => {
            void refreshSidebar()
            openThread(threadId)
          }}
          open
          origin={dialog.origin}
        />
      ) : null}
      {messageActions.dialogs}
    </AppShell>
  )
}

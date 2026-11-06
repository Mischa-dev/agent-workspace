import {
  useCallback,
  useMemo,
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

import { requestJson } from '../api/client'
import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { CatchUpButton } from '../ai/CatchUpButton'
import { useAuthenticatedSession } from '../auth/session'
import { useProjectExperience } from '../experience/layout'
import { useProject } from '../projects/hooks'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import {
  ProjectPageError,
  ProjectPageLoading,
} from '../projects/ProjectPageStates'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { ConversationHeader } from '../conversations/ConversationHeader'
import { ThreadStatusButton, ThreadStatusDialog } from '../conversations/threadBoard'
import { useProjectConversationSidebar } from '../conversations/hooks'
import { useMessageActions } from '../conversations/MessageActions'
import { useReplyTarget } from '../conversations/replyTarget'
import { OriginMessage } from '../conversations/OriginMessage'
import { useConversationTools } from '../later/useConversationTools'
import { PinnedBar } from '../memory/ConversationMemory'
import { ReplyStatusList } from '../conversations/ReplyStatus'
import { groupReplyRequests } from '../conversations/replyStatusModel'
import {
  ConversationTimeline,
  OlderMessagesButton,
  PendingMessages,
  StreamingMessageRow,
  TimelineEntries,
} from '../conversations/Timeline'
import { useAutoMarkSeen } from '../conversations/useAutoMarkSeen'
import { useArchiveConversation, useRestoreConversation } from '../conversations/useArchiveConversation'
import { useFirstUnread } from '../conversations/useFirstUnread'
import { useMessageSender } from '../conversations/useMessageSender'
import { SessionLinks } from '../launch/SessionLinks'
import { useLiveOlderEntries } from '../conversations/useLiveOlderEntries'
import { usePageTitle } from '../shell/documentTitleModel'
import { retryReplyRequest, searchMentionableActors, stopReplyRequest } from '../conversations/api'
import type { ConversationMessage, ConversationTimelineEntry } from '../conversations/types'
import { useConversationRealtime } from '../realtime/realtimeContext'
import { useSetWorkItemThreadLink } from '../work-items/hooks'
import { searchWorkItemReferences } from '../work-items/api'
import { uploadFile } from '../files/api'
import { Banner, Button, ConversationIntro, ErrorState, IconButton, MessageSkeleton, SegmentedControl } from '../ui'
import { ApprovalCard, RunBadge, RunCard, ThreadDetails } from './ThreadPresentation'
import { RunCardActions, RunCardNotes, RunComposer } from './ThreadRunControls'
import { runAgent } from './runAgent'
import { normalizeTimeline, useThread } from './threadTimeline'
import { useThreadRunActions } from './useThreadRunActions'

const NO_ENTRIES: ConversationTimelineEntry[] = []

export function ThreadPage() {
  const session = useAuthenticatedSession()
  const { projectId = '', threadId = '' } = useParams()
  const [searchParams] = useSearchParams()
  const includeArchived = searchParams.get('includeArchived') === 'true'
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const project = useProject(projectId)
  const thread = useThread(projectId, threadId, includeArchived)
  const developerDetail = useProjectExperience(projectId).shows('tech.ids')
  const workItemLink = useSetWorkItemThreadLink()
  const realtime = useConversationRealtime(threadId)
  const timeline = thread.timeline.data
  const verifiedTimeline = thread.routeMatches ? timeline : undefined
  usePageTitle(verifiedTimeline?.conversation.name ?? null)
  const activeRun = verifiedTimeline?.activeRun ?? null
  const latestTerminalRun = verifiedTimeline?.latestTerminalRun ?? null
  const viewer = { actorId: session.user.actorId, displayName: session.user.displayName }
  // Kept per thread: the page stays mounted from one thread to the next.
  const [detailsFor, setDetailsFor] = useState<string | null>(null)
  const detailsOpen = detailsFor === threadId
  const setDetailsOpen = (open: boolean) => setDetailsFor(open ? threadId : null)
  const [statusFor, setStatusFor] = useState<string | null>(null)
  const [modeFor, setModeFor] = useState<{ threadId: string; mode: 'message' | 'run' }>({ threadId, mode: 'message' })
  const composerMode = modeFor.threadId === threadId ? modeFor.mode : 'message'
  const setComposerMode = (mode: 'message' | 'run') => setModeFor({ threadId, mode })
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
        throw new Error('Older timeline page crossed its project boundary.')
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
  const retryAgent = (requestId: string) => retryReplyRequest(requestId).then(() => thread.timeline.refetch())
  const stopAgent = verifiedTimeline?.capabilities.sendMessage
    ? (requestId: string) => stopReplyRequest(requestId).then(() => thread.timeline.refetch())
    : undefined
  const olderCursor = older.cursor
  const unread = useFirstUnread({
    conversationId: threadId,
    entries,
    fresh: thread.timeline.isFetchedAfterMount,
    hasOlder: Boolean(olderCursor),
    page: verifiedTimeline,
    viewerId: viewer.actorId,
  })

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

  const runActions = useThreadRunActions({ entries, includeArchived, projectId, threadId, timeline: verifiedTimeline })
  const { busyAction } = runActions
  const archiveThread = useArchiveConversation({
    conversationId: threadId,
    kind: 'thread',
    label: `“${verifiedTimeline?.conversation.name ?? 'thread'}”`,
    leaveTo: verifiedTimeline?.conversation.parentChannelId
      ? `/projects/${encodeURIComponent(projectId)}/channels/${encodeURIComponent(verifiedTimeline.conversation.parentChannelId)}`
      : `/projects/${encodeURIComponent(projectId)}`,
    projectId,
  })
  const restoreThread = useRestoreConversation({
    conversationId: threadId,
    kind: 'thread',
    label: `“${verifiedTimeline?.conversation.name ?? 'thread'}”`,
    projectId,
  })

  const searchActors = async (query: string) => (await searchMentionableActors(projectId, query)).actors
  const searchWorkItems = (query: string) => searchWorkItemReferences(projectId, query)
  const reply = useReplyTarget(threadId, verifiedTimeline?.conversation.parentKind === 'channel' ? projectId : null)
  const startReply = reply.start
  const messageActions = useMessageActions({
    projectId,
    // A reply is written in the message composer, so Reply switches to it
    // from the Run composer.
    onReply: startReply
      ? (message: ConversationMessage) => {
          setComposerMode('message')
          startReply(message)
        }
      : undefined,
    conversationId: threadId,
    viewerId: session.user.actorId,
    capabilities: verifiedTimeline?.capabilities,
    searchActors,
    searchWorkItems,
    taskProject: verifiedTimeline?.conversation.parentKind === 'channel',
    onReactionsChanged: older.patchReactions,
    onChanged: (entry) => {
      older.replaceEntry(entry)
      void thread.timeline.refetch()
    },
    onMarkedUnread: (entry) => unread.markUnreadFrom(entry.sequence),
  })

  const sender = useMessageSender({
    conversationId: threadId,
    kind: 'thread',
    onSent: () => void onSeen(),
    queryKey: queryKeys.timeline(projectId, threadId, includeArchived),
  })

  const tools = useConversationTools({
    canPost: Boolean(verifiedTimeline?.capabilities.sendMessage && !verifiedTimeline.conversation.archivedAt),
    conversationId: threadId,
    kind: 'thread',
    latestMessageId: entries.at(-1)?.message?.id,
    projectId,
  })

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
        <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
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
  // Streamed text comes from the run that's working now.
  const writer = runAgent(activeRun)
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
        unlinking={workItemLink.isPending}
        workItems={verifiedTimeline.linkedWorkItems}
      />
    ) : undefined

  // An active run without a stop button has no actions, so its card has no footer.
  const cardActions =
    runForCard && (!activeRun || activeRun.capabilities.interrupt) ? (
      <RunCardActions actions={runActions} activeRun={activeRun} latestTerminalRun={latestTerminalRun} />
    ) : null
  const cardNotes = latestTerminalRun && !activeRun ? <RunCardNotes run={latestTerminalRun} /> : null

  const modeSwitch =
    canMessage && showRunComposer ? (
      <SegmentedControl
        label="What to send"
        onChange={setComposerMode}
        options={[
          { value: 'message', label: 'Message', icon: 'message' },
          { value: 'run', label: 'Run an agent', icon: 'terminal' },
        ]}
        value={mode}
      />
    ) : null

  const threadStatus = verifiedTimeline?.conversation.threadStatus ?? 'Open'
  const statusThread = { id: threadId, name: threadName, status: threadStatus, outcome: verifiedTimeline?.conversation.threadOutcome ?? null }
  const canArchive = Boolean(verifiedTimeline?.capabilities.archive)
  const canRestore = Boolean(verifiedTimeline?.capabilities.restore)
  const restore = () => {
    if (verifiedTimeline && !restoreThread.isPending) restoreThread.mutate(verifiedTimeline.conversation.version ?? 1)
  }
  const streaming = Object.entries(realtime.streaming)
  const originId = verifiedTimeline?.conversation.originMessageId ?? null

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
              <SessionLinks conversationId={threadId} projectId={projectId} />
              {/* Open is the usual state: the chip shows only once it says something. */}
              {threadStatus.toLowerCase() !== 'open' ? (
                <ThreadStatusButton onSaved={() => void thread.timeline.refetch()} thread={statusThread} />
              ) : null}
              <CatchUpButton conversationId={threadId} name={threadName} />
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
          verifiedTimeline
            ? [
                { label: parentChannel ? `Back to #${parentChannel.name}` : 'Back to the channel', icon: 'arrow-left', onSelect: () => navigate(channelRoute) },
                ...(verifiedTimeline.capabilities.sendMessage && !archived
                  ? [{ label: 'Rename or set status…', icon: 'pencil' as const, onSelect: () => setStatusFor(threadId) }]
                  : []),
                { separator: true },
                ...tools.levelMenu,
                ...(canRestore ? [{ separator: true as const }, { label: 'Restore thread', icon: 'undo' as const, onSelect: restore }] : []),
                ...(canArchive && !archived
                  ? [
                      { separator: true as const },
                      // Archiving can be undone (from the toast, or Restore later), so it happens at once.
                      {
                        label: 'Archive thread',
                        icon: 'archive' as const,
                        danger: true,
                        onSelect: () => archiveThread.mutate(verifiedTimeline.conversation.version ?? 1),
                      },
                    ]
                  : []),
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
      {verifiedTimeline ? <PinnedBar canEdit={canMessage} conversationId={threadId} projectId={projectId} /> : null}

      <ConversationTimeline
        busy={thread.timeline.isPending || older.loading}
        hasOlder={Boolean(olderCursor)}
        key={threadId}
        label={`Messages in ${threadName}`}
        olderKey={older.olderCount}
        onLoadOlder={() => void older.loadOlder()}
        revision={`${verifiedTimeline?.highestSequence ?? 0}:${sender.pending.length}:${streaming.map(([, text]) => text).join('').length}:${pendingApproval?.id ?? ''}:${runForCard?.state ?? ''}`}
        stickKey={sender.sent}
        unread={unread.unread}
      >
        {olderCursor ? <OlderMessagesButton busy={older.loading} onClick={() => void older.loadOlder()} /> : null}
        {older.error ? (
          <Banner onRetry={() => void older.loadOlder()} title="Earlier messages didn’t load." tone="danger">
            {older.error}
          </Banner>
        ) : null}
        {thread.timeline.isPending ? <MessageSkeleton label="Loading thread" /> : null}
        {thread.timeline.isError ? (
          <ErrorState error={thread.timeline.error} onRetry={() => void thread.timeline.refetch()} title="This thread didn’t load" />
        ) : null}
        {verifiedTimeline && !olderCursor ? (
          originId ? (
            <OriginMessage
              channelHref={channelRoute}
              channelName={parentChannel ? `#${parentChannel.name}` : null}
              direct={verifiedTimeline.conversation.parentKind !== 'channel'}
              originId={originId}
              parentId={parentId}
              projectId={projectId}
              viewerId={viewer.actorId}
            />
          ) : (
            <ConversationIntro icon="messages" title={threadName}>
              {verifiedTimeline.capabilities.workerInvoke
                ? 'Ask an agent to make a change here and it runs in its own workspace, with every step, approval and result kept in this thread.'
                : 'Everything said here stays with this thread.'}
            </ConversationIntro>
          )
        ) : null}
        <TimelineEntries
          conversationId={threadId}
          agentName={(runId) => runAgent([activeRun, latestTerminalRun].find((run) => run?.id === runId)).name}
          entries={entries}
          firstUnreadSequence={unread.firstUnreadSequence}
          onOpenHistory={messageActions.openHistory}
          renderEditor={messageActions.editorFor}
          renderExtras={(entry) =>
            entry.message && !entry.message.removed ? (
              <ReplyStatusList onRetry={retryAgent} onStop={stopAgent} requests={requestsByMessage.get(entry.message.id) ?? []} />
            ) : null
          }
          renderReactions={messageActions.reactionsFor}
          renderToolbar={messageActions.toolbarFor}
          viewerId={viewer.actorId}
        />
        {trailingRequests.length ? (
          <div className="aw-replies--trailing">
            <ReplyStatusList onRetry={retryAgent} onStop={stopAgent} requests={trailingRequests} />
          </div>
        ) : null}
        <PendingMessages entries={entries} items={sender.pending} viewer={viewer} />
        {streaming.map(([messageId, text]) => (
          <StreamingMessageRow key={messageId} name={writer.name} seed={writer.seed} text={text} />
        ))}
        {runForCard ? (
          <div className="aw-timeline__card">
            <RunCard actions={cardActions} developerDetail={developerDetail} notes={cardNotes} run={runForCard} />
          </div>
        ) : null}
        {pendingApproval ? (
          <div className="aw-timeline__card">
            <ApprovalCard
              approval={pendingApproval}
              busyAction={busyAction}
              canRespond={pendingApproval.capabilities.respond}
              onRespond={runActions.respond}
              run={[activeRun, latestTerminalRun].find((run) => run?.id === pendingApproval.agentRunId) ?? activeRun}
            />
          </div>
        ) : null}
      </ConversationTimeline>

      {mode === 'message' && canMessage ? (
        <StructuredComposer
          autoFocus={searchParams.get('compose') === '1'}
          draftKey={threadId}
          focusOnOpen
          label={`Message ${threadName}`}
          outbox={{
            kind: 'message',
            conversationId: threadId,
            conversationKind: 'thread',
            href: `/projects/${encodeURIComponent(projectId)}/threads/${encodeURIComponent(threadId)}`,
          }}
          onEditLast={() => messageActions.editLast(entries)}
          onReplyChange={reply.set}
          onSubmit={sender.send}
          replyTo={reply.target}
          placeholder={`Reply in ${threadName}`}
          searchActors={searchActors}
          searchWorkItems={searchWorkItems}
          toolbar={modeSwitch}
          upload={uploadFile({ kind: 'conversation', conversationId: threadId })}
          {...tools.composer}
        />
      ) : null}
      {mode === 'run' && showRunComposer ? <RunComposer actions={runActions} activeRun={activeRun} modeSwitch={modeSwitch} /> : null}
      {verifiedTimeline && archived ? (
        <StructuredComposer
          disabledReason={
            <>
              This thread is archived. You can read it, but no one can post or start runs.
              {canRestore ? (
                <Button busy={restoreThread.isPending} icon="undo" onClick={restore} size="sm">
                  Restore thread
                </Button>
              ) : null}
            </>
          }
          onSubmit={async () => undefined}
          searchActors={async () => []}
        />
      ) : null}

      {messageActions.dialogs}
      {statusFor === threadId && verifiedTimeline ? (
        <ThreadStatusDialog onClose={() => setStatusFor(null)} onSaved={() => void thread.timeline.refetch()} thread={statusThread} />
      ) : null}
    </AppShell>
  )
}

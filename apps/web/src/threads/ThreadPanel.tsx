import { useCallback, useMemo } from 'react'
import { Link, useLocation } from 'react-router-dom'

import { requestJson } from '../api/client'
import { queryKeys } from '../api/queryKeys'
import { retryReplyRequest, stopReplyRequest } from '../conversations/api'
import { useAutoMarkSeen } from '../conversations/useAutoMarkSeen'
import { useMessageActions } from '../conversations/MessageActions'
import { useReplyTarget } from '../conversations/replyTarget'
import { OriginMessage } from '../conversations/OriginMessage'
import { ReplyStatusList } from '../conversations/ReplyStatus'
import { groupReplyRequests } from '../conversations/replyStatusModel'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { ThreadStatusButton } from '../conversations/threadBoard'
import { useRefreshThreads } from '../conversations/threadModel'
import {
  ConversationTimeline,
  OlderMessagesButton,
  PendingMessages,
  StreamingMessageRow,
  TimelineEntries,
} from '../conversations/Timeline'
import type { ConversationActor, ConversationTimelineEntry, ConversationWorkItemReference } from '../conversations/types'
import { useFirstUnread } from '../conversations/useFirstUnread'
import { useLiveOlderEntries } from '../conversations/useLiveOlderEntries'
import { useMessageSender } from '../conversations/useMessageSender'
import { useConversationTools } from '../later/useConversationTools'
import { useOptionalRealtime } from '../realtime/realtimeContext'
import { Banner, ErrorState, Icon, IconButton, MessageSkeleton } from '../ui'
import { uploadFile } from '../files/api'
import { ApprovalCard, RunBadge } from './ThreadPresentation'
import { runAgent } from './runAgent'
import { normalizeTimeline, useThread } from './threadTimeline'
import { useThreadRunActions } from './useThreadRunActions'

const NO_ENTRIES: ConversationTimelineEntry[] = []

/**
 * A thread open beside its channel, DM or group, so you keep your place. The
 * one thread component: project channels and direct conversations both use
 * it. It shows the message the thread started from, the thread's messages
 * (with earlier ones as you scroll up), what an agent is writing, and an
 * approval waiting on you, so "Needs you" can be answered here.
 */
export function ThreadPanel({
  fullPageHref,
  members,
  onClose,
  parentId,
  parentName,
  projectId,
  searchActors,
  searchWorkItems,
  threadId,
  viewer,
}: {
  /** Where the thread opens as a page, with its runs and details. */
  fullPageHref?: string
  /** Who can be mentioned, for a DM or group. Project channels pass searchActors instead. */
  members?: ConversationActor[]
  onClose: () => void
  parentId: string
  /** "#general", or nothing for a DM or group. */
  parentName?: string
  projectId: string
  searchActors?: (query: string) => Promise<ConversationActor[]>
  searchWorkItems?: (query: string) => Promise<ConversationWorkItemReference[]>
  threadId: string
  viewer: { actorId: string; displayName: string }
}) {
  const location = useLocation()
  const thread = useThread(projectId, threadId, false)
  const refreshThreads = useRefreshThreads()
  const realtime = useOptionalRealtime()
  const page = thread.timeline.data
  // The id comes from the URL: show only a thread that belongs to this parent.
  const valid = Boolean(thread.routeMatches && page?.conversation.parentChannelId === parentId)
  const timeline = valid ? page : undefined
  const older = useLiveOlderEntries({
    conversationId: threadId,
    newestEntries: timeline?.entries ?? NO_ENTRIES,
    newestCursor: timeline?.nextCursor ?? null,
    loadPage: async (cursor) => {
      const result = normalizeTimeline(
        await requestJson<unknown>(
          `/api/v1/conversations/${encodeURIComponent(threadId)}/timeline?${new URLSearchParams({ limit: '50', cursor })}`,
        ),
      )
      if (result.conversation.id !== threadId || result.conversation.projectId !== projectId || result.conversation.kind !== 'thread') {
        throw new Error('Older timeline page crossed its project boundary.')
      }
      return result
    },
  })
  const entries = older.entries
  const unread = useFirstUnread({
    conversationId: threadId,
    entries,
    fresh: thread.timeline.isFetchedAfterMount,
    hasOlder: Boolean(older.cursor),
    page: timeline,
    viewerId: viewer.actorId,
  })
  const searchMembers =
    searchActors ??
    (async (query: string) => {
      const needle = query.trim().toLowerCase()
      return (members ?? []).filter(
        (member) =>
          member.id !== viewer.actorId &&
          (!needle || member.handle.toLowerCase().includes(needle) || member.displayName.toLowerCase().includes(needle)),
      )
    })
  const archived = Boolean(timeline?.conversation.archivedAt)
  const canPost = Boolean(timeline?.capabilities.sendMessage && !archived)
  const threadHref = `/projects/${encodeURIComponent(projectId)}/threads/${encodeURIComponent(threadId)}`
  const reply = useReplyTarget(threadId)
  const messageActions = useMessageActions({
    projectId,
    onReply: reply.start,
    conversationId: threadId,
    direct: !searchWorkItems,
    viewerId: viewer.actorId,
    capabilities: timeline?.capabilities,
    // A link to a message here opens the thread itself, where it is.
    linkFor: (messageId) => `${threadHref}#message-${messageId}`,
    searchActors: searchMembers,
    searchWorkItems,
    taskProject: Boolean(searchWorkItems),
    onReactionsChanged: older.patchReactions,
    onChanged: (entry) => {
      older.replaceEntry(entry)
      void thread.timeline.refetch()
      void refreshThreads(parentId)
    },
    onMarkedUnread: (entry) => unread.markUnreadFrom(entry.sequence),
  })
  const sender = useMessageSender({
    conversationId: threadId,
    kind: 'thread',
    onSent: () => void refreshThreads(parentId),
    queryKey: queryKeys.timeline(projectId, threadId, false),
  })
  const tools = useConversationTools({
    canPost,
    conversationId: threadId,
    kind: 'thread',
    latestMessageId: entries.at(-1)?.message?.id,
    projectId,
  })
  const runActions = useThreadRunActions({ entries, includeArchived: false, projectId, threadId, timeline })
  const activeRun = timeline?.activeRun ?? null
  const latestTerminalRun = timeline?.latestTerminalRun ?? null
  const pendingApproval = timeline?.approvals.find((approval) => approval.status === 'pending')
  const writer = runAgent(activeRun)

  const requestsByMessage = useMemo(() => groupReplyRequests(timeline?.replyRequests ?? []), [timeline?.replyRequests])
  const retryAgent = (requestId: string) => retryReplyRequest(requestId).then(() => thread.timeline.refetch())
  const stopAgent = timeline?.capabilities.sendMessage
    ? (requestId: string) => stopReplyRequest(requestId).then(() => thread.timeline.refetch())
    : undefined

  const onSeen = useCallback(() => refreshThreads(parentId), [parentId, refreshThreads])
  useAutoMarkSeen(
    {
      conversationId: threadId,
      highestSequence: timeline?.highestSequence ?? 0,
      highestSeenSequence: timeline?.readState.highestSeenSequence ?? 0,
      ready: Boolean(timeline?.capabilities.markSeen && entries.at(-1)?.sequence === timeline.highestSequence),
    },
    onSeen,
  )

  const conversation = timeline?.conversation ?? null
  const streaming = Object.entries(realtime?.streams[threadId] ?? {})
  return (
    <aside aria-label="Thread" className="context-drawer aw-threadpanel">
      <div className="aw-threadpanel__head">
        <h2 className="aw-threadpanel__title">{conversation?.name ?? 'Thread'}</h2>
        <RunBadge run={activeRun} />
        {conversation ? (
          <ThreadStatusButton
            onSaved={() => {
              void thread.timeline.refetch()
              void refreshThreads(parentId)
            }}
            quietWhenOpen
            thread={{
              id: conversation.id,
              name: conversation.name,
              status: conversation.threadStatus ?? 'Open',
              outcome: conversation.threadOutcome,
            }}
          />
        ) : null}
        {fullPageHref ? (
          <Link aria-label="Open as a page" className="aw-iconbtn aw-iconbtn--sm" title="Open as a page" to={fullPageHref}>
            <Icon name="external" size={16} />
          </Link>
        ) : null}
        <IconButton className="aw-panelclose" icon="x" label="Close thread" onClick={onClose} size="sm" />
      </div>
      {conversation?.threadOutcome ? <p className="aw-threadpanel__outcome">{conversation.threadOutcome}</p> : null}
      <ConversationTimeline
        busy={thread.timeline.isPending || older.loading}
        followLinks={false}
        hasOlder={Boolean(older.cursor)}
        key={threadId}
        label="Thread messages"
        olderKey={older.olderCount}
        onLoadOlder={() => void older.loadOlder()}
        revision={`${timeline?.highestSequence ?? 0}:${sender.pending.length}:${streaming.map(([, text]) => text).join('').length}:${pendingApproval?.id ?? ''}`}
        stickKey={sender.sent}
        unread={unread.unread}
      >
        {thread.timeline.isPending ? <MessageSkeleton label="Loading thread" rows={2} /> : null}
        {thread.timeline.isError ? (
          <ErrorState error={thread.timeline.error} onRetry={() => void thread.timeline.refetch()} title="This thread didn’t load" />
        ) : null}
        {page && !valid ? <ErrorState title="This thread isn’t in this conversation" /> : null}
        {timeline && !older.cursor ? (
          <OriginMessage
            // Beside its channel already: the link only scrolls the channel to it.
            channelHref={`${location.pathname}${location.search}`}
            channelName={parentName}
            direct={!parentName}
            originId={timeline.conversation.originMessageId}
            parentId={parentId}
            projectId={projectId}
            viewerId={viewer.actorId}
          />
        ) : null}
        {older.cursor ? <OlderMessagesButton busy={older.loading} onClick={() => void older.loadOlder()} /> : null}
        {older.error ? (
          <Banner onRetry={() => void older.loadOlder()} title="Earlier messages didn’t load." tone="danger">
            {older.error}
          </Banner>
        ) : null}
        <TimelineEntries
          agentName={(runId) => runAgent([activeRun, latestTerminalRun].find((run) => run?.id === runId)).name}
          conversationId={threadId}
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
        <PendingMessages entries={entries} items={sender.pending} viewer={viewer} />
        {streaming.map(([messageId, text]) => (
          <StreamingMessageRow key={messageId} name={writer.name} seed={writer.seed} text={text} />
        ))}
        {pendingApproval ? (
          <div className="aw-timeline__card">
            <ApprovalCard
              approval={pendingApproval}
              busyAction={runActions.busyAction}
              canRespond={pendingApproval.capabilities.respond}
              onRespond={runActions.respond}
              run={[activeRun, latestTerminalRun].find((run) => run?.id === pendingApproval.agentRunId) ?? activeRun}
            />
          </div>
        ) : null}
      </ConversationTimeline>
      {canPost ? (
        <StructuredComposer
          draftKey={threadId}
          focusOnOpen
          label="Reply in thread"
          outbox={{ kind: 'message', conversationId: threadId, conversationKind: 'thread', href: threadHref }}
          onEditLast={() => messageActions.editLast(entries)}
          onReplyChange={reply.set}
          onSubmit={sender.send}
          replyTo={reply.target}
          placeholder="Reply in thread"
          searchActors={searchMembers}
          searchWorkItems={searchWorkItems}
          upload={uploadFile({ kind: 'conversation', conversationId: threadId })}
          {...tools.composer}
        />
      ) : timeline && archived ? (
        <StructuredComposer
          disabledReason="This thread is archived. You can read it, but no one can post."
          onSubmit={async () => undefined}
          searchActors={async () => []}
        />
      ) : null}
      {messageActions.dialogs}
    </aside>
  )
}

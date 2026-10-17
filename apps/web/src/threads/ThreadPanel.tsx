import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router-dom'

import { getConversationTimeline, retryReplyRequest, sendConversationMessage, stopReplyRequest } from '../conversations/api'
import { useAutoMarkSeen } from '../conversations/useAutoMarkSeen'
import type { ComposerSubmission } from '../conversations/composerModel'
import { useConversationTimeline } from '../conversations/hooks'
import { useMessageActions } from '../conversations/MessageActions'
import { ReplyStatusList } from '../conversations/ReplyStatus'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { ThreadStatusButton } from '../conversations/threadBoard'
import { useRefreshThreads } from '../conversations/threadModel'
import { PendingMessageRow, TimelineEntries } from '../conversations/Timeline'
import type { ConversationActor, ConversationTimelineEntry, ConversationWorkItemReference, ReplyRequest } from '../conversations/types'
import { useLiveOlderEntries } from '../conversations/useLiveOlderEntries'
import { Button, ErrorState, Icon, IconButton, MessageSkeleton } from '../ui'
import { uploadFile } from '../files/api'

const NO_ENTRIES: ConversationTimelineEntry[] = []

/**
 * A thread open beside its channel, DM or group, so you keep your place. The
 * one thread component: project channels and direct conversations both use it.
 */
export function ThreadPanel({
  fullPageHref,
  members,
  onClose,
  parentId,
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
  projectId: string
  searchActors?: (query: string) => Promise<ConversationActor[]>
  searchWorkItems?: (query: string) => Promise<ConversationWorkItemReference[]>
  threadId: string
  viewer: { actorId: string; displayName: string }
}) {
  const timeline = useConversationTimeline(projectId, threadId)
  const refreshThreads = useRefreshThreads()
  const [pending, setPending] = useState<{ key: string; text?: string }[]>([])
  const scroller = useRef<HTMLDivElement>(null)
  const page = timeline.data
  // The id comes from the URL: show only a thread that belongs to this parent.
  const valid =
    page?.conversation.id === threadId &&
    page.conversation.kind === 'thread' &&
    page.conversation.projectId === projectId &&
    page.conversation.parentChannelId === parentId
  // The panel shows the newest page only; this keeps it current between refetches.
  const live = useLiveOlderEntries({
    conversationId: threadId,
    newestEntries: valid && page ? page.entries : NO_ENTRIES,
    newestCursor: null,
    loadPage: (cursor) => getConversationTimeline(threadId, cursor),
  })
  const searchMembers = searchActors ?? (async (query: string) => {
    const needle = query.trim().toLowerCase()
    return (members ?? []).filter(
      (member) =>
        member.id !== viewer.actorId &&
        (!needle || member.handle.toLowerCase().includes(needle) || member.displayName.toLowerCase().includes(needle)),
    )
  })
  const messageActions = useMessageActions({
    projectId,
    conversationId: threadId,
    viewerId: viewer.actorId,
    capabilities: valid ? page?.capabilities : null,
    searchActors: searchMembers,
    searchWorkItems,
    onReactionsChanged: live.patchReactions,
    onChanged: (entry) => {
      live.replaceEntry(entry)
      void timeline.refetch()
      void refreshThreads(parentId)
    },
  })

  const requestsByMessage = useMemo(() => {
    const byMessage = new Map<string, ReplyRequest[]>()
    for (const request of valid ? page?.replyRequests ?? [] : []) {
      byMessage.set(request.messageId, [...(byMessage.get(request.messageId) ?? []), request])
    }
    return byMessage
  }, [page?.replyRequests, valid])

  useEffect(() => {
    const element = scroller.current
    if (element) element.scrollTop = element.scrollHeight
  }, [page?.highestSequence, pending.length])

  const onSeen = useCallback(() => refreshThreads(parentId), [parentId, refreshThreads])
  useAutoMarkSeen(
    {
      conversationId: threadId,
      highestSequence: valid && page ? page.highestSequence : 0,
      highestSeenSequence: valid && page ? page.readState.highestSeenSequence : 0,
      ready: Boolean(valid && page?.capabilities.markSeen),
    },
    onSeen,
  )

  const send = async (submission: ComposerSubmission) => {
    setPending((current) => [...current, { key: submission.idempotencyKey, text: submission.text }])
    try {
      await sendConversationMessage({ conversationId: threadId, kind: 'thread', ...submission })
      await timeline.refetch()
      void refreshThreads(parentId)
    } finally {
      setPending((current) => current.filter((item) => item.key !== submission.idempotencyKey))
    }
  }

  const conversation = valid ? page?.conversation : null
  return (
    <aside aria-label="Thread" className="context-drawer aw-threadpanel">
      <div className="aw-threadpanel__head">
        <h2 className="aw-threadpanel__title">{conversation?.name ?? 'Thread'}</h2>
        {conversation ? (
          <ThreadStatusButton
            onSaved={() => {
              void timeline.refetch()
              void refreshThreads(parentId)
            }}
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
        <IconButton icon="x" label="Close thread" onClick={onClose} size="sm" />
      </div>
      {conversation?.threadOutcome ? <p className="aw-threadpanel__outcome">{conversation.threadOutcome}</p> : null}
      <div aria-label="Thread messages" className="aw-threadpanel__messages" ref={scroller} role="log">
        {timeline.isPending ? <MessageSkeleton label="Loading thread" rows={2} /> : null}
        {timeline.isError ? (
          <ErrorState action={<Button onClick={() => void timeline.refetch()}>Try again</Button>} title="This thread didn’t load" />
        ) : null}
        {page && !valid ? <ErrorState title="This thread isn’t in this conversation" /> : null}
        <TimelineEntries
          conversationId={threadId}
          entries={live.entries}
          onOpenHistory={messageActions.openHistory}
          renderEditor={messageActions.editorFor}
          renderExtras={(entry) =>
            entry.message && !entry.message.removed ? (
              <ReplyStatusList
                onRetry={(requestId) => void retryReplyRequest(requestId).then(() => timeline.refetch())}
                onStop={page?.capabilities.sendMessage ? (requestId) => stopReplyRequest(requestId).then(() => timeline.refetch()) : undefined}
                requests={requestsByMessage.get(entry.message.id) ?? []}
              />
            ) : null
          }
          renderReactions={messageActions.reactionsFor}
          renderToolbar={messageActions.toolbarFor}
          viewerId={viewer.actorId}
        />
        {pending.map((item) => (
          <PendingMessageRow continued key={item.key} name={viewer.displayName} seed={viewer.actorId} text={item.text} />
        ))}
      </div>
      {valid && page?.capabilities.sendMessage ? (
        <StructuredComposer
          draftKey={threadId}
          upload={uploadFile({ kind: 'conversation', conversationId: threadId })}
          label="Reply in thread"
          onSubmit={send}
          placeholder="Reply in thread"
          searchActors={searchMembers}
          searchWorkItems={searchWorkItems}
        />
      ) : null}
      {messageActions.dialogs}
    </aside>
  )
}

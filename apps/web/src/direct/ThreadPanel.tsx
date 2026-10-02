import { useEffect, useMemo, useRef, useState } from 'react'

import { getConversationTimeline, markConversationSeen, retryReplyRequest, sendConversationMessage } from '../conversations/api'
import type { ComposerSubmission } from '../conversations/composerModel'
import { useConversationTimeline } from '../conversations/hooks'
import { useMessageActions } from '../conversations/MessageActions'
import { ReplyStatusList } from '../conversations/ReplyStatus'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { ThreadStatusButton } from '../conversations/threadBoard'
import { useRefreshThreads } from '../conversations/threadModel'
import { PendingMessageRow, TimelineEntries } from '../conversations/Timeline'
import type { ConversationActor, ConversationTimelineEntry, ReplyRequest } from '../conversations/types'
import { useLiveOlderEntries } from '../conversations/useLiveOlderEntries'
import { Button, ErrorState, IconButton, MessageSkeleton } from '../ui'

const NO_ENTRIES: ConversationTimelineEntry[] = []

/** A thread under a DM or group, open beside the conversation so you keep your place. */
export function ThreadPanel({
  members,
  onClose,
  parentId,
  projectId,
  threadId,
  viewer,
}: {
  members: ConversationActor[]
  onClose: () => void
  parentId: string
  projectId: string
  threadId: string
  viewer: { actorId: string; displayName: string }
}) {
  const timeline = useConversationTimeline(projectId, threadId)
  const refreshThreads = useRefreshThreads()
  const [pending, setPending] = useState<{ key: string; text?: string }[]>([])
  const scroller = useRef<HTMLDivElement>(null)
  const marked = useRef(0)
  const page = timeline.data
  const valid = page?.conversation.id === threadId && page.conversation.kind === 'thread'
  // The panel shows the newest page only; this keeps it current between refetches.
  const live = useLiveOlderEntries({
    conversationId: threadId,
    newestEntries: valid && page ? page.entries : NO_ENTRIES,
    newestCursor: null,
    loadPage: (cursor) => getConversationTimeline(threadId, cursor),
  })
  const searchMembers = async (query: string) => {
    const needle = query.trim().toLowerCase()
    return members.filter(
      (member) =>
        member.id !== viewer.actorId &&
        (!needle || member.handle.toLowerCase().includes(needle) || member.displayName.toLowerCase().includes(needle)),
    )
  }
  const messageActions = useMessageActions({
    projectId,
    conversationId: threadId,
    viewerId: viewer.actorId,
    capabilities: valid ? page?.capabilities : null,
    searchActors: searchMembers,
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

  useEffect(() => {
    if (!valid || !page || !page.capabilities.markSeen) return
    if (page.highestSequence <= page.readState.highestSeenSequence || page.highestSequence <= marked.current) return
    marked.current = page.highestSequence
    void markConversationSeen({ conversationId: threadId, highestSeenSequence: page.highestSequence })
      .then(() => refreshThreads(parentId))
      .catch(() => {
        marked.current = 0
      })
  }, [page, parentId, refreshThreads, threadId, valid])

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
        <IconButton icon="x" label="Close thread" onClick={onClose} size="sm" />
      </div>
      {conversation?.threadOutcome ? <p className="aw-threadpanel__outcome">{conversation.threadOutcome}</p> : null}
      <div aria-label="Thread messages" className="aw-threadpanel__messages" ref={scroller} role="log">
        {timeline.isPending ? <MessageSkeleton label="Loading thread" rows={2} /> : null}
        {timeline.isError ? (
          <ErrorState action={<Button onClick={() => void timeline.refetch()}>Try again</Button>} title="This thread didn’t load" />
        ) : null}
        <TimelineEntries
          entries={live.entries}
          onOpenHistory={messageActions.openHistory}
          renderEditor={messageActions.editorFor}
          renderExtras={(entry) =>
            entry.message && !entry.message.removed ? (
              <ReplyStatusList
                onRetry={(requestId) => void retryReplyRequest(requestId).then(() => timeline.refetch())}
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
          label="Reply in thread"
          onSubmit={send}
          placeholder="Reply in thread"
          searchActors={searchMembers}
        />
      ) : null}
      {messageActions.dialogs}
    </aside>
  )
}

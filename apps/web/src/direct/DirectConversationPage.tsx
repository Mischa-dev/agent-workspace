import { useCallback, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Navigate, useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { toastError } from '../app/mutations'
import { useAuthenticatedSession } from '../auth/session'
import { continueConversation, getConversationTimeline, retryReplyRequest, stopReplyRequest } from '../conversations/api'
import { ConversationHeader } from '../conversations/ConversationHeader'
import { useConversationTimeline } from '../conversations/useConversationTimeline'
import { useMessageActions } from '../conversations/MessageActions'
import { useReplyTarget } from '../conversations/replyTarget'
import { useConversationTools } from '../later/useConversationTools'
import { ReplyStatusList } from '../conversations/ReplyStatus'
import { groupReplyRequests } from '../conversations/replyStatusModel'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { ThreadBoard } from '../conversations/threadBoard'
import { ThreadChip } from '../conversations/ThreadChip'
import { threadChipFacts } from '../conversations/threadChipModel'
import { useRefreshThreads, useThreads } from '../conversations/threadModel'
import { ThreadDialog } from '../conversations/ThreadDialog'
import { ConversationTimeline, OlderMessagesButton, PendingMessages, TimelineEntries } from '../conversations/Timeline'
import type { ConversationActor, ConversationTimelineEntry, ThreadSummary } from '../conversations/types'
import { useAutoMarkSeen } from '../conversations/useAutoMarkSeen'
import { useFirstUnread } from '../conversations/useFirstUnread'
import { useLiveOlderEntries } from '../conversations/useLiveOlderEntries'
import { useMessageSender } from '../conversations/useMessageSender'
import { PinnedBar } from '../memory/ConversationMemory'
import { AppShell } from '../shell/AppShell'
import {
  ActorTag,
  Avatar,
  AvatarStack,
  Banner,
  Button,
  ConfirmDialog,
  ConversationIntro,
  ErrorState,
  IconButton,
  MessageSkeleton,
  SelectField,
  actorKind,
  describeError,
  showToast,
} from '../ui'
import { addGroupMember, conversationTitle, directQueryKey, removeGroupMember, useDirectOverview } from './api'
import { usePageTitle } from '../shell/documentTitleModel'
import { ThreadPanel } from '../threads/ThreadPanel'
import { uploadFile } from '../files/api'

const NO_ENTRIES: ConversationTimelineEntry[] = []

type SidePanel = 'members' | 'threads'

function listNames(names: string[]) {
  if (names.length <= 1) return names.join('')
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

export function DirectConversationPage() {
  const session = useAuthenticatedSession()
  const { conversationId = '' } = useParams()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const overview = useDirectOverview()
  const projectId = overview.data?.projectId ?? ''
  const timeline = useConversationTimeline(projectId, conversationId)
  // Kept per conversation: the page stays mounted from one DM to the next.
  const [side, setSide] = useState<{ conversationId: string; panel: SidePanel } | null>(null)
  const panel = side?.conversationId === conversationId ? side.panel : null
  const togglePanel = (next: SidePanel) => setSide(panel === next ? null : { conversationId, panel: next })
  const [threadDialog, setThreadDialog] = useState<{
    conversationId: string
    origin: { messageId: string; text: string; author: string }
  } | null>(null)
  const [leaving, setLeaving] = useState<string | null>(null)
  const [continuing, setContinuing] = useState(false)
  const [searchParams, setSearchParams] = useSearchParams()
  const openThreadId = searchParams.get('thread')
  const threads = useThreads(projectId, conversationId)
  const refreshThreads = useRefreshThreads()
  const page = timeline.data
  const viewerId = session.user.actorId
  const viewer = { actorId: viewerId, displayName: session.user.displayName }
  const summary = overview.data?.conversations.find((conversation) => conversation.id === conversationId)
  const members = summary?.members ?? []
  const valid = page?.conversation.id === conversationId && (page.conversation.kind === 'dm' || page.conversation.kind === 'group')
  const isGroup = (summary?.kind ?? page?.conversation.kind) === 'group'
  const other = members.find((member) => member.id !== viewerId)

  const older = useLiveOlderEntries({
    conversationId,
    newestEntries: valid && page ? page.entries : NO_ENTRIES,
    newestCursor: valid ? (page?.nextCursor ?? null) : null,
    loadPage: async (cursor) => {
      const result = await getConversationTimeline(conversationId, cursor)
      if (result.conversation.id !== conversationId || (result.conversation.kind !== 'dm' && result.conversation.kind !== 'group')) {
        throw new Error('Older messages came back for a different conversation.')
      }
      return result
    },
  })
  const entries = older.entries
  const olderCursor = older.cursor
  const unread = useFirstUnread({
    conversationId,
    entries,
    fresh: timeline.isFetchedAfterMount,
    hasOlder: Boolean(olderCursor),
    page: valid ? page : null,
    viewerId,
  })

  const requestsByMessage = useMemo(() => groupReplyRequests(valid ? (page?.replyRequests ?? []) : []), [page?.replyRequests, valid])
  const threadsByMessage = useMemo(() => {
    const byMessage = new Map<string, ThreadSummary[]>()
    for (const thread of threads.data ?? []) {
      if (!thread.originMessageId) continue
      byMessage.set(thread.originMessageId, [...(byMessage.get(thread.originMessageId) ?? []), thread])
    }
    return byMessage
  }, [threads.data])
  const openThread = (threadId: string | null) => {
    const next = new URLSearchParams(searchParams)
    if (threadId) next.set('thread', threadId)
    else next.delete('thread')
    setSearchParams(next)
  }
  const lastEntry = entries.at(-1)
  const waitingForContinue = page?.agentTurnsRemaining === 0 && lastEntry?.message?.author.kind === 'external_agent'

  const onSeen = useCallback(() => queryClient.invalidateQueries({ queryKey: directQueryKey }), [queryClient])
  useAutoMarkSeen(
    {
      conversationId,
      highestSequence: valid && page ? page.highestSequence : 0,
      highestSeenSequence: valid && page ? page.readState.highestSeenSequence : 0,
      ready: Boolean(valid && page?.capabilities.markSeen && entries.at(-1)?.sequence === page.highestSequence),
    },
    onSeen,
  )

  const searchMembers = async (query: string) => {
    const needle = query.trim().toLowerCase()
    return members.filter(
      (member) =>
        member.id !== viewerId &&
        (!needle || member.handle.toLowerCase().includes(needle) || member.displayName.toLowerCase().includes(needle)),
    )
  }

  const canPost = Boolean(valid && page?.capabilities.sendMessage)
  const startThread = (origin: { messageId: string; text: string; author: string }) => setThreadDialog({ conversationId, origin })
  const reply = useReplyTarget(conversationId)
  const messageActions = useMessageActions({
    projectId,
    onReply: reply.start,
    conversationId,
    direct: true,
    viewerId,
    capabilities: valid ? page?.capabilities : null,
    searchActors: searchMembers,
    onReactionsChanged: older.patchReactions,
    onChanged: (entry) => {
      older.replaceEntry(entry)
      void timeline.refetch()
    },
    onMarkedUnread: (entry) => unread.markUnreadFrom(entry.sequence),
    primaryFor: (message) => {
      const existing = threadsByMessage.get(message.id)?.[0]
      if (existing) return <IconButton icon="messages" label="Open thread" onClick={() => openThread(existing.id)} size="sm" />
      if (!canPost) return null
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

  const sender = useMessageSender({
    conversationId,
    kind: page?.conversation.kind ?? (isGroup ? 'group' : 'dm'),
    onSent: () => void queryClient.invalidateQueries({ queryKey: directQueryKey }),
    queryKey: queryKeys.timeline(projectId, conversationId, false),
  })

  const title = summary ? conversationTitle(summary, viewerId) : (page?.conversation.name ?? 'Conversation')
  usePageTitle(summary || page ? title : null)

  const tools = useConversationTools({
    canPost,
    conversationId,
    kind: isGroup ? 'group' : 'dm',
    latestMessageId: entries.at(-1)?.message?.id,
    projectId,
  })

  const letContinue = () => {
    setContinuing(true)
    void continueConversation(conversationId)
      .then(() => timeline.refetch())
      .catch((reason: unknown) => toastError('The agents weren’t asked to continue', reason))
      .finally(() => setContinuing(false))
  }

  if (isApiError(timeline.error) && timeline.error.status === 404) {
    return <Navigate replace to="/messages" />
  }

  const otherKind = other ? actorKind(other.kind) : 'person'
  const memberPeople = members.map((member) => ({ id: member.id, name: member.displayName, kind: actorKind(member.kind) }))
  const othersNames = members.filter((member) => member.id !== viewerId).map((member) => member.displayName)

  const threadsButton = (
    <IconButton
      icon="messages"
      label={panel === 'threads' ? 'Hide threads' : 'Threads'}
      onClick={() => {
        openThread(null)
        togglePanel('threads')
      }}
      pressed={panel === 'threads'}
      side="bottom"
    />
  )

  const header = isGroup ? (
    <ConversationHeader
      actions={
        <>
          {threadsButton}
          <button
            aria-expanded={panel === 'members'}
            aria-label={`${members.length} members`}
            className="aw-convhead__members"
            onClick={() => {
              openThread(null)
              togglePanel('members')
            }}
            type="button"
          >
            <AvatarStack max={3} people={memberPeople} size={22} />
            <span>{members.length}</span>
          </button>
        </>
      }
      icon="users"
      menu={[
        ...tools.levelMenu,
        ...(summary ? [{ separator: true as const }, { label: 'Leave group…', icon: 'log-out' as const, danger: true, onSelect: () => setLeaving(conversationId) }] : []),
      ]}
      subtitle={othersNames.length ? `With ${listNames(othersNames)}` : undefined}
      tag={tools.levelMark}
      title={title}
    />
  ) : (
    <ConversationHeader
      actions={threadsButton}
      lead={other ? <Avatar kind={otherKind} name={other.displayName} seed={other.id} size={24} /> : undefined}
      menu={tools.levelMenu}
      subtitle={other ? `@${other.handle}` : undefined}
      tag={
        <>
          <ActorTag kind={otherKind} />
          {tools.levelMark}
        </>
      }
      title={title}
    />
  )

  const intro = isGroup ? (
    <ConversationIntro icon="users" title={title}>
      A group with {listNames(othersNames.length ? ['you', ...othersNames] : ['you'])}. Agents answer when they’re asked or mentioned, and can pass when
      they have nothing to add.
    </ConversationIntro>
  ) : other ? (
    <ConversationIntro
      mark={<Avatar kind={otherKind} name={other.displayName} seed={other.id} size={64} />}
      title={
        <>
          {other.displayName}
          <ActorTag kind={otherKind} />
        </>
      }
    >
      {otherKind === 'agent'
        ? `Messages here ask ${other.displayName} for a reply. You’ll see when it picks one up, and it can pass if it has nothing to add.`
        : `This is the beginning of your conversation with ${other.displayName}. Only the two of you can see it.`}
    </ConversationIntro>
  ) : null

  const dialog = threadDialog?.conversationId === conversationId ? threadDialog : null

  return (
    <AppShell
      context={
        openThreadId && projectId ? (
          <ThreadPanel
            key={openThreadId}
            members={members}
            onClose={() => openThread(null)}
            parentId={conversationId}
            projectId={projectId}
            threadId={openThreadId}
            viewer={viewer}
          />
        ) : panel === 'threads' ? (
          <ThreadBoard
            conversationId={conversationId}
            projectId={projectId}
            onClose={() => setSide(null)}
            onOpen={(thread) => {
              setSide(null)
              openThread(thread.id)
            }}
          />
        ) : isGroup && panel === 'members' ? (
          <GroupMembers
            conversationId={conversationId}
            members={members}
            onClose={() => setSide(null)}
            onLeave={() => setLeaving(conversationId)}
            people={overview.data?.people ?? []}
            viewerId={viewerId}
          />
        ) : undefined
      }
      contextKey={openThreadId ?? panel ?? undefined}
      direct
      mainClassName="aw-main--conversation"
      onContextDismiss={() => {
        if (openThreadId) openThread(null)
        setSide(null)
      }}
      user={session.user}
    >
      {header}
      {tools.banner}
      {valid && projectId ? <PinnedBar canEdit={canPost} conversationId={conversationId} direct projectId={projectId} /> : null}
      <ConversationTimeline
        busy={timeline.isPending || older.loading}
        hasOlder={Boolean(olderCursor)}
        key={conversationId}
        label={`Messages with ${title}`}
        olderKey={older.olderCount}
        onLoadOlder={() => void older.loadOlder()}
        revision={`${page?.highestSequence ?? 0}:${sender.pending.length}:${page?.replyRequests.map((request) => request.status).join(',') ?? ''}`}
        stickKey={sender.sent}
        unread={unread.unread}
      >
        {olderCursor ? <OlderMessagesButton busy={older.loading} onClick={() => void older.loadOlder()} /> : null}
        {older.error ? (
          <Banner onRetry={() => void older.loadOlder()} title="Earlier messages didn’t load." tone="danger">
            {older.error}
          </Banner>
        ) : null}
        {overview.isError ? (
          <ErrorState error={overview.error} onRetry={() => void overview.refetch()} title="Your messages didn’t load" />
        ) : (overview.isPending || timeline.isPending) && !timeline.isError ? (
          <MessageSkeleton />
        ) : null}
        {timeline.isError ? (
          <ErrorState error={timeline.error} onRetry={() => void timeline.refetch()} title="This conversation didn’t load" />
        ) : null}
        {valid && !olderCursor ? intro : null}
        <TimelineEntries
          conversationId={conversationId}
          entries={entries}
          firstUnreadSequence={unread.firstUnreadSequence}
          onOpenHistory={messageActions.openHistory}
          renderEditor={messageActions.editorFor}
          renderExtras={(entry) =>
            entry.message ? (
              <>
                {entry.message.removed ? null : (
                  <ReplyStatusList
                    onRetry={(requestId) => retryReplyRequest(requestId).then(() => timeline.refetch())}
                    onStop={page?.capabilities.sendMessage ? (requestId) => stopReplyRequest(requestId).then(() => timeline.refetch()) : undefined}
                    requests={requestsByMessage.get(entry.message.id) ?? []}
                  />
                )}
                {(threadsByMessage.get(entry.message.id) ?? []).map((thread) => {
                  const facts = threadChipFacts(thread)
                  return facts ? (
                    <ThreadChip current={thread.id === openThreadId} key={thread.id} onOpen={() => openThread(thread.id)} thread={facts} />
                  ) : null
                })}
              </>
            ) : null
          }
          renderReactions={messageActions.reactionsFor}
          renderToolbar={messageActions.toolbarFor}
          viewerId={viewerId}
        />
        <PendingMessages entries={entries} items={sender.pending} viewer={viewer} />
      </ConversationTimeline>
      {waitingForContinue ? (
        <Banner
          action={
            <Button busy={continuing} onClick={letContinue} size="sm" variant="primary">
              Let them continue
            </Button>
          }
          className="aw-conv__banner"
          title="The agents have used their turns."
          tone="agent"
        >
          They’ll wait here until someone steps in.
        </Banner>
      ) : null}
      {canPost ? (
        <StructuredComposer
          autoFocus={searchParams.get('compose') === '1'}
          draftKey={conversationId}
          focusOnOpen={!openThreadId}
          label={`Message ${title}`}
          outbox={{
            kind: 'message',
            conversationId,
            conversationKind: isGroup ? 'group' : 'dm',
            href: `/messages/${encodeURIComponent(conversationId)}`,
          }}
          onEditLast={() => messageActions.editLast(entries)}
          onReplyChange={reply.set}
          onSubmit={sender.send}
          replyTo={reply.target}
          placeholder={`Message ${title}`}
          searchActors={searchMembers}
          upload={uploadFile({ kind: 'conversation', conversationId })}
          {...tools.composer}
        />
      ) : null}
      {messageActions.dialogs}
      {dialog ? (
        <ThreadDialog
          channelId={conversationId}
          onClose={() => setThreadDialog(null)}
          onCreated={(threadId) => {
            void refreshThreads(conversationId)
            openThread(threadId)
          }}
          open
          origin={dialog.origin}
        />
      ) : null}
      <LeaveGroupDialog
        conversationId={conversationId}
        onClose={() => setLeaving(null)}
        onLeft={() => navigate('/messages')}
        open={leaving === conversationId}
        title={title}
        viewerId={viewerId}
      />
    </AppShell>
  )
}

/** Leaving can't be undone from here: only someone still in the group can add you back. */
function LeaveGroupDialog({
  conversationId,
  onClose,
  onLeft,
  open,
  title,
  viewerId,
}: {
  conversationId: string
  onClose: () => void
  onLeft: () => void
  open: boolean
  title: string
  viewerId: string
}) {
  const queryClient = useQueryClient()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const leave = () => {
    setBusy(true)
    setError(null)
    void removeGroupMember(conversationId, viewerId)
      .then(async () => {
        await queryClient.invalidateQueries({ queryKey: directQueryKey })
        showToast({ tone: 'success', title: `Left ${title}` })
        onClose()
        onLeft()
      })
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(false))
  }
  return (
    <ConfirmDialog busy={busy} confirmLabel="Leave group" error={error} onClose={onClose} onConfirm={leave} open={open} title="Leave this group?">
      You’ll stop seeing its messages. Someone still in it can add you back.
    </ConfirmDialog>
  )
}

function GroupMembers({
  conversationId,
  members,
  onClose,
  onLeave,
  people,
  viewerId,
}: {
  conversationId: string
  members: ConversationActor[]
  onClose: () => void
  onLeave: () => void
  people: ConversationActor[]
  viewerId: string
}) {
  const queryClient = useQueryClient()
  const [adding, setAdding] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const memberIds = new Set(members.map((member) => member.id))
  const candidates = people.filter((person) => !memberIds.has(person.id))
  const refresh = () => queryClient.invalidateQueries({ queryKey: directQueryKey })

  const add = (person: ConversationActor, quiet = false) => {
    setBusy(person.id)
    return addGroupMember(conversationId, person.id)
      .then(async () => {
        await refresh()
        if (!quiet) showToast({ tone: 'success', title: `Added ${person.displayName}` })
      })
      .catch((reason: unknown) => toastError(`${person.displayName} wasn’t added`, reason))
      .finally(() => setBusy(null))
  }

  // Someone removed can be added back, so it happens at once with Undo.
  const remove = (member: ConversationActor) => {
    setBusy(member.id)
    void removeGroupMember(conversationId, member.id)
      .then(async () => {
        await refresh()
        showToast({ tone: 'success', title: `Removed ${member.displayName}`, onUndo: () => add(member, true) })
      })
      .catch((reason: unknown) => toastError(`${member.displayName} wasn’t removed`, reason))
      .finally(() => setBusy(null))
  }

  return (
    <aside aria-label="Group members" className="context-drawer aw-members">
      <div className="aw-members__head">
        <h2>Members</h2>
        <IconButton className="aw-panelclose" icon="x" label="Close members" onClick={onClose} size="sm" />
      </div>
      <ul className="aw-members__list">
        {members.map((member) => {
          const kind = actorKind(member.kind)
          const you = member.id === viewerId
          return (
            <li className="aw-members__row" key={member.id}>
              <Avatar kind={kind} name={member.displayName} seed={member.id} size={32} />
              <span className="aw-members__name">
                {member.displayName}
                {you ? <span className="aw-members__you"> (you)</span> : null}
              </span>
              <ActorTag kind={kind} />
              <Button
                aria-label={you ? 'Leave this group' : `Remove ${member.displayName}`}
                busy={busy === member.id}
                disabled={busy !== null}
                onClick={() => (you ? onLeave() : remove(member))}
                size="sm"
                variant="ghost"
              >
                {you ? 'Leave' : 'Remove'}
              </Button>
            </li>
          )
        })}
      </ul>
      {candidates.length > 0 ? (
        <form
          className="aw-members__add"
          onSubmit={(event) => {
            event.preventDefault()
            const person = candidates.find((candidate) => candidate.id === adding)
            if (person) void add(person).then(() => setAdding(''))
          }}
        >
          <SelectField
            hint="They’ll see everything said here so far."
            id="group-add-member"
            label="Add someone"
            onChange={(event) => setAdding(event.target.value)}
            value={adding}
          >
            <option value="">Choose a person or agent…</option>
            {candidates.map((person) => (
              <option key={person.id} value={person.id}>
                {person.displayName}
              </option>
            ))}
          </SelectField>
          <Button busy={busy === adding && adding !== ''} disabled={!adding || busy !== null} type="submit" variant="primary">
            Add
          </Button>
        </form>
      ) : null}
    </aside>
  )
}

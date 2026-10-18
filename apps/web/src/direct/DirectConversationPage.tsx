import { useCallback, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Navigate, useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import {
  continueConversation,
  getConversationTimeline,
  retryReplyRequest,
  stopReplyRequest,
  sendConversationMessage,
} from '../conversations/api'
import { ConversationHeader } from '../conversations/ConversationHeader'
import type { ComposerSubmission } from '../conversations/composerModel'
import { useConversationTimeline } from '../conversations/hooks'
import { useMessageActions } from '../conversations/MessageActions'
import { useConversationTools } from '../later/useConversationTools'
import { ReplyStatusList } from '../conversations/ReplyStatus'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { ThreadBoard, ThreadStatusChip } from '../conversations/threadBoard'
import { useRefreshThreads, useThreads } from '../conversations/threadModel'
import { ThreadDialog } from '../conversations/ThreadDialog'
import { ConversationTimeline, OlderMessagesButton, PendingMessageRow, TimelineEntries } from '../conversations/Timeline'
import type { ConversationActor, ConversationTimelineEntry, ReplyRequest, ThreadSummary } from '../conversations/types'
import { useAutoMarkSeen } from '../conversations/useAutoMarkSeen'
import { useLiveOlderEntries } from '../conversations/useLiveOlderEntries'
import { MessageMemoryActions, PinnedBar } from '../memory/ConversationMemory'
import { AppShell } from '../shell/AppShell'
import {
  ActorTag,
  Avatar,
  AvatarStack,
  Banner,
  Button,
  ConversationIntro,
  ErrorState,
  Icon,
  IconButton,
  MessageSkeleton,
  SelectField,
  actorKind,
} from '../ui'
import { addGroupMember, conversationTitle, directQueryKey, removeGroupMember, useDirectOverview } from './api'
import { usePageTitle } from '../shell/documentTitleModel'
import { ThreadPanel } from '../threads/ThreadPanel'

const NO_ENTRIES: ConversationTimelineEntry[] = []

function listNames(names: string[]) {
  if (names.length <= 1) return names.join('')
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

export function DirectConversationPage() {
  const session = useAuthenticatedSession()
  const { conversationId = '' } = useParams()
  const queryClient = useQueryClient()
  const overview = useDirectOverview()
  const projectId = overview.data?.projectId ?? ''
  const timeline = useConversationTimeline(projectId, conversationId)
  const [pending, setPending] = useState<{ key: string; text?: string }[]>([])
  const [membersOpen, setMembersOpen] = useState(false)
  const [boardOpen, setBoardOpen] = useState(false)
  const [threadDialog, setThreadDialog] = useState<{ messageId: string; text: string; author: string } | null>(null)
  const [searchParams, setSearchParams] = useSearchParams()
  const openThreadId = searchParams.get('thread')
  const threads = useThreads(projectId, conversationId)
  const refreshThreads = useRefreshThreads()
  const page = timeline.data
  const viewerId = session.user.actorId
  const summary = overview.data?.conversations.find((conversation) => conversation.id === conversationId)
  const members = summary?.members ?? []
  const valid = page?.conversation.id === conversationId && (page.conversation.kind === 'dm' || page.conversation.kind === 'group')
  const isGroup = (summary?.kind ?? page?.conversation.kind) === 'group'
  const other = members.find((member) => member.id !== viewerId)

  const older = useLiveOlderEntries({
    conversationId,
    newestEntries: valid && page ? page.entries : NO_ENTRIES,
    newestCursor: valid ? page?.nextCursor ?? null : null,
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

  const requestsByMessage = useMemo(() => {
    const byMessage = new Map<string, ReplyRequest[]>()
    for (const request of valid ? page?.replyRequests ?? [] : []) {
      byMessage.set(request.messageId, [...(byMessage.get(request.messageId) ?? []), request])
    }
    return byMessage
  }, [page?.replyRequests, valid])
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
      ready: Boolean(valid && page?.capabilities.markSeen),
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

  const messageActions = useMessageActions({
    projectId,
    conversationId,
    viewerId,
    capabilities: valid ? page?.capabilities : null,
    searchActors: searchMembers,
    onReactionsChanged: older.patchReactions,
    onChanged: (entry) => {
      older.replaceEntry(entry)
      void timeline.refetch()
    },
    actionsFor: (message) => {
      if (!valid || !page?.capabilities.sendMessage) return null
      const existing = threadsByMessage.get(message.id)?.[0]
      return (
        <>
          {existing ? (
            <IconButton icon="messages" label="Open thread" onClick={() => openThread(existing.id)} size="sm" />
          ) : (
            <IconButton
              icon="messages"
              label="Start a thread"
              onClick={() => setThreadDialog({ messageId: message.id, text: message.plainText, author: message.author.displayName })}
              size="sm"
            />
          )}
          <MessageMemoryActions conversationId={conversationId} message={message} projectId={projectId} />
        </>
      )
    },
  })

  const send = async (submission: ComposerSubmission) => {
    if (!page) return
    setPending((current) => [...current, { key: submission.idempotencyKey, text: submission.text }])
    try {
      await sendConversationMessage({ conversationId, kind: page.conversation.kind, ...submission })
      await timeline.refetch()
      void queryClient.invalidateQueries({ queryKey: directQueryKey })
    } finally {
      setPending((current) => current.filter((item) => item.key !== submission.idempotencyKey))
    }
  }

  const title = summary ? conversationTitle(summary, viewerId) : page?.conversation.name ?? 'Conversation'
  usePageTitle(summary || page ? title : null)

  const tools = useConversationTools({
    canPost: Boolean(valid && page?.capabilities.sendMessage),
    conversationId,
    kind: isGroup ? 'group' : 'dm',
    latestMessageId: entries.at(-1)?.message?.id,
    projectId,
  })

  if (isApiError(timeline.error) && timeline.error.status === 404) {
    return <Navigate replace to="/messages" />
  }

  const otherKind = other ? actorKind(other.kind) : 'person'
  const memberPeople = members.map((member) => ({ id: member.id, name: member.displayName, kind: actorKind(member.kind) }))
  const othersNames = members.filter((member) => member.id !== viewerId).map((member) => member.displayName)

  const threadsButton = (
    <IconButton
      icon="messages"
      label={boardOpen ? 'Hide threads' : 'Threads'}
      onClick={() => {
        openThread(null)
        setMembersOpen(false)
        setBoardOpen((open) => !open)
      }}
      pressed={boardOpen}
      side="bottom"
    />
  )


  const header = isGroup ? (
    <ConversationHeader
      actions={
        <>
          {threadsButton}
          <button
            aria-expanded={membersOpen}
            aria-label={`${members.length} members`}
            className="aw-convhead__members"
            onClick={() => {
              openThread(null)
              setBoardOpen(false)
              setMembersOpen((open) => !open)
            }}
            type="button"
          >
            <AvatarStack max={3} people={memberPeople} size={22} />
            <span>{members.length}</span>
          </button>
        </>
      }
      icon="users"
      menu={tools.levelMenu}
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
            viewer={{ actorId: viewerId, displayName: session.user.displayName }}
          />
        ) : boardOpen ? (
          <ThreadBoard
            conversationId={conversationId}
            projectId={projectId}
            onClose={() => setBoardOpen(false)}
            onOpen={(thread) => {
              setBoardOpen(false)
              openThread(thread.id)
            }}
          />
        ) : isGroup && membersOpen ? (
          <GroupMembers
            conversationId={conversationId}
            members={members}
            onClose={() => setMembersOpen(false)}
            people={overview.data?.people ?? []}
            viewerId={viewerId}
          />
        ) : undefined
      }
      contextKey={openThreadId ?? (boardOpen ? 'threads' : membersOpen ? 'members' : undefined)}
      direct
      mainClassName="aw-main--conversation"
      onContextDismiss={() => {
        if (openThreadId) openThread(null)
        setBoardOpen(false)
        setMembersOpen(false)
      }}
      user={session.user}
    >
      {header}
      {tools.banner}
      {valid && projectId ? <PinnedBar canEdit={Boolean(page?.capabilities.sendMessage)} conversationId={conversationId} direct projectId={projectId} /> : null}
      <ConversationTimeline busy={timeline.isPending} label={`Messages with ${title}`} revision={`${page?.highestSequence ?? 0}:${pending.length}:${page?.replyRequests.map((request) => request.status).join(',') ?? ''}`}>
        {olderCursor ? <OlderMessagesButton busy={older.loading} onClick={() => void older.loadOlder()} /> : null}
        {older.error ? <Banner tone="danger">{older.error}</Banner> : null}
        {overview.isPending || timeline.isPending ? <MessageSkeleton /> : null}
        {timeline.isError && !isApiError(timeline.error) ? (
          <ErrorState action={<Button onClick={() => void timeline.refetch()}>Try again</Button>} title="This conversation didn’t load">
            Check your connection and try again.
          </ErrorState>
        ) : null}
        {valid && !olderCursor ? intro : null}
        <TimelineEntries
          entries={entries}
          onOpenHistory={messageActions.openHistory}
          renderEditor={messageActions.editorFor}
          renderExtras={(entry) =>
            entry.message ? (
              <>
                {entry.message.removed ? null : (
                  <ReplyStatusList
                    onRetry={(requestId) => void retryReplyRequest(requestId).then(() => timeline.refetch())}
                    onStop={page?.capabilities.sendMessage ? (requestId) => stopReplyRequest(requestId).then(() => timeline.refetch()) : undefined}
                    requests={requestsByMessage.get(entry.message.id) ?? []}
                  />
                )}
                {(threadsByMessage.get(entry.message.id) ?? []).map((thread) => (
                  <button className="aw-threadlink" key={thread.id} onClick={() => openThread(thread.id)} type="button">
                    <Icon name="messages" size={14} />
                    <span className="aw-threadlink__name">{thread.name}</span>
                    <ThreadStatusChip status={thread.status} />
                    {thread.outcome ? <span className="aw-threadlink__state">{thread.outcome}</span> : null}
                    <Icon className="aw-threadlink__chev" name="chevron-right" size={14} />
                  </button>
                ))}
              </>
            ) : null
          }
          renderReactions={messageActions.reactionsFor}
          renderToolbar={messageActions.toolbarFor}
          viewerId={viewerId}
        />
        {pending.map((item, index) => (
          <PendingMessageRow
            continued={index > 0 || entries.at(-1)?.message?.author.id === viewerId}
            key={item.key}
            name={session.user.displayName}
            seed={viewerId}
            text={item.text}
          />
        ))}
      </ConversationTimeline>
      {waitingForContinue ? (
        <Banner
          action={
            <Button onClick={() => void continueConversation(conversationId).then(() => timeline.refetch())} size="sm" variant="primary">
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
      {valid && page?.capabilities.sendMessage ? (
        <StructuredComposer
          autoFocus={searchParams.get('compose') === '1'}
          draftKey={conversationId}
          label={`Message ${title}`}
          onSubmit={send}
          placeholder={`Message ${title}`}
          searchActors={searchMembers}
          {...tools.composer}
        />
      ) : null}
      {messageActions.dialogs}
      {threadDialog ? (
        <ThreadDialog
          channelId={conversationId}
          onClose={() => setThreadDialog(null)}
          onCreated={(threadId) => {
            void refreshThreads(conversationId)
            openThread(threadId)
          }}
          open
          origin={threadDialog}
        />
      ) : null}
    </AppShell>
  )
}

function GroupMembers({
  conversationId,
  members,
  onClose,
  people,
  viewerId,
}: {
  conversationId: string
  members: ConversationActor[]
  onClose: () => void
  people: ConversationActor[]
  viewerId: string
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [adding, setAdding] = useState('')
  const [error, setError] = useState<string | null>(null)
  const memberIds = new Set(members.map((member) => member.id))
  const candidates = people.filter((person) => !memberIds.has(person.id))
  const run = (action: Promise<void>, after?: () => void) => {
    setError(null)
    void action
      .then(async () => {
        await queryClient.invalidateQueries({ queryKey: directQueryKey })
        after?.()
      })
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : 'That didn’t work. Try again.'))
  }
  return (
    <aside aria-label="Group members" className="context-drawer aw-members">
      <div className="aw-members__head">
        <h2>Members</h2>
        <IconButton icon="x" label="Close members" onClick={onClose} size="sm" />
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
                onClick={() => run(removeGroupMember(conversationId, member.id), you ? () => navigate('/messages') : undefined)}
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
            if (adding) run(addGroupMember(conversationId, adding), () => setAdding(''))
          }}
        >
          <SelectField
            hint="They'll see everything said here so far."
            id="group-add-member"
            label="Add someone"
            onChange={(event) => setAdding(event.target.value)} value={adding}>
            <option value="">Choose a person or agent…</option>
            {candidates.map((person) => (
              <option key={person.id} value={person.id}>
                {person.displayName}
              </option>
            ))}
          </SelectField>
          <Button disabled={!adding} type="submit" variant="primary">
            Add
          </Button>
        </form>
      ) : null}
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </aside>
  )
}

import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, Navigate, useNavigate, useParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import {
  continueConversation,
  getConversationTimeline,
  markConversationSeen,
  retryReplyRequest,
  sendConversationMessage,
} from '../conversations/api'
import type { ComposerSubmission } from '../conversations/composerModel'
import { useConversationTimeline } from '../conversations/hooks'
import { MessageContent } from '../conversations/MessageContent'
import { StructuredComposer } from '../conversations/StructuredComposer'
import { SystemTimelineEntry } from '../conversations/SystemTimelineEntry'
import type {
  ConversationActor,
  ConversationTimelineEntry,
  ReplyRequest,
} from '../conversations/types'
import { useConversationRealtime } from '../realtime/realtimeContext'
import { AppShell } from '../shell/AppShell'
import {
  addGroupMember,
  conversationTitle,
  directQueryKey,
  removeGroupMember,
  useDirectOverview,
} from './api'

export function DirectConversationPage() {
  const session = useAuthenticatedSession()
  const { conversationId = '' } = useParams()
  const queryClient = useQueryClient()
  const overview = useDirectOverview()
  const projectId = overview.data?.projectId ?? ''
  const timeline = useConversationTimeline(projectId, conversationId)
  const realtime = useConversationRealtime(conversationId)
  const [older, setOlder] = useState<{
    conversationId: string
    entries: ConversationTimelineEntry[]
    nextCursor: string | null
  }>({ conversationId: '', entries: [], nextCursor: null })
  const [pending, setPending] = useState<string[]>([])
  const markedSeen = useRef({ conversationId: '', sequence: 0 })
  const page = timeline.data
  const summary = overview.data?.conversations.find(
    (conversation) => conversation.id === conversationId,
  )
  const members = summary?.members ?? []
  const valid =
    page?.conversation.id === conversationId &&
    (page.conversation.kind === 'dm' || page.conversation.kind === 'group')
  const olderCursor =
    older.conversationId === conversationId
      ? older.nextCursor
      : page?.nextCursor ?? null

  const entries = useMemo(() => {
    const bySequence = new Map<number, ConversationTimelineEntry>()
    const historical =
      older.conversationId === conversationId ? older.entries : []
    for (const entry of [...historical, ...(valid ? page?.entries ?? [] : [])]) {
      bySequence.set(entry.sequence, entry)
    }
    return [...bySequence.values()].sort((a, b) => a.sequence - b.sequence)
  }, [conversationId, older, page?.entries, valid])

  const requestsByMessage = useMemo(() => {
    const byMessage = new Map<string, ReplyRequest[]>()
    for (const request of valid ? page?.replyRequests ?? [] : []) {
      byMessage.set(request.messageId, [
        ...(byMessage.get(request.messageId) ?? []),
        request,
      ])
    }
    return byMessage
  }, [page?.replyRequests, valid])
  const lastEntry = entries.at(-1)
  const waitingForContinue =
    page?.agentTurnsRemaining === 0 &&
    lastEntry?.message?.author.kind === 'external_agent'

  useEffect(() => {
    if (
      !valid ||
      !page ||
      !page.capabilities.markSeen ||
      page.highestSequence <= page.readState.highestSeenSequence ||
      (markedSeen.current.conversationId === conversationId &&
        page.highestSequence <= markedSeen.current.sequence)
    ) {
      return
    }
    markedSeen.current = { conversationId, sequence: page.highestSequence }
    void markConversationSeen({
      conversationId,
      highestSeenSequence: page.highestSequence,
    })
      .then(() => queryClient.invalidateQueries({ queryKey: directQueryKey }))
      .catch(() => {
        markedSeen.current = { conversationId: '', sequence: 0 }
      })
  }, [conversationId, page, queryClient, valid])

  const loadOlder = async () => {
    if (!olderCursor) return
    const result = await getConversationTimeline(conversationId, olderCursor)
    setOlder((current) => ({
      conversationId,
      entries: [
        ...result.entries,
        ...(current.conversationId === conversationId ? current.entries : []),
      ],
      nextCursor: result.nextCursor,
    }))
  }

  const send = async (submission: ComposerSubmission) => {
    if (!page) return
    setPending((current) => [...current, submission.idempotencyKey])
    try {
      await sendConversationMessage({
        conversationId,
        kind: page.conversation.kind,
        ...submission,
      })
      await timeline.refetch()
      void queryClient.invalidateQueries({ queryKey: directQueryKey })
    } finally {
      setPending((current) =>
        current.filter((key) => key !== submission.idempotencyKey),
      )
    }
  }

  if (isApiError(timeline.error) && timeline.error.status === 404) {
    return <Navigate replace to="/messages" />
  }

  const title = summary
    ? conversationTitle(summary, session.user.actorId)
    : page?.conversation.name ?? 'Conversation'

  return (
    <AppShell
      context={
        summary?.kind === 'group' ? (
          <GroupMembers
            conversationId={conversationId}
            members={members}
            people={overview.data?.people ?? []}
            viewerId={session.user.actorId}
          />
        ) : undefined
      }
      direct
      mainClassName="workspace conversation-workspace"
      user={session.user}
    >
      <header className="workspace-heading">
        <div>
          <span className="section-label">
            {summary?.kind === 'group' ? 'Group' : 'Direct message'}
          </span>
          <h1 id="workspace-heading">{title}</h1>
          {summary?.kind === 'group' ? (
            <p className="conversation-members">
              {members.map((member) => member.displayName).join(', ')}
            </p>
          ) : null}
        </div>
        <div className="header-status">
          <span className="connection-state" data-connection={realtime.connection}>
            <span aria-hidden="true">●</span>{' '}
            {realtime.connection === 'live' ? 'Live' : 'Reconnecting'}
          </span>
        </div>
      </header>

      <section
        aria-busy={timeline.isPending}
        aria-label="Conversation timeline"
        className="timeline conversation-timeline"
      >
        {olderCursor ? (
          <button
            className="load-older-button"
            onClick={() => void loadOlder()}
            type="button"
          >
            Load older Messages
          </button>
        ) : null}
        {overview.isPending || timeline.isPending ? (
          <p role="status">Loading…</p>
        ) : null}
        {timeline.isError && !isApiError(timeline.error) ? (
          <div className="error-state" role="alert">
            <h2>Conversation unavailable</h2>
            <button onClick={() => void timeline.refetch()} type="button">
              Try again
            </button>
          </div>
        ) : null}
        {valid && entries.length === 0 ? (
          <div className="empty-state">
            <h2>Say hello.</h2>
            <p>Only the people in this conversation can see it.</p>
          </div>
        ) : null}
        {entries.map((entry) => (
          <div key={entry.id}>
            <TimelineEntry entry={entry} />
            {entry.message ? (
              <ReplyStatus
                onRetry={(requestId) =>
                  void retryReplyRequest(requestId).then(() => timeline.refetch())
                }
                requests={requestsByMessage.get(entry.message.id) ?? []}
              />
            ) : null}
          </div>
        ))}
        {waitingForContinue ? (
          <div className="reply-continue" role="status">
            <span>
              The agents have used their turns for now, so they stop here
              until you step in.
            </span>
            <button
              className="secondary-button"
              onClick={() =>
                void continueConversation(conversationId).then(() =>
                  timeline.refetch(),
                )
              }
              type="button"
            >
              Let them continue
            </button>
          </div>
        ) : null}
        {pending.map((key) => (
          <article aria-label="Message sending" className="message pending-message" key={key}>
            <header>
              <strong>@{session.user.loginName}</strong>
              <span>Sending…</span>
            </header>
          </article>
        ))}
      </section>

      {valid && page?.capabilities.sendMessage ? (
        <StructuredComposer
          onSubmit={send}
          placeholder={`Message ${title}…`}
          searchActors={async (query) => {
            const needle = query.trim().toLowerCase()
            return members.filter(
              (member) =>
                member.id !== session.user.actorId &&
                (!needle ||
                  member.handle.toLowerCase().includes(needle) ||
                  member.displayName.toLowerCase().includes(needle)),
            )
          }}
        />
      ) : null}
    </AppShell>
  )
}

function TimelineEntry({ entry }: { entry: ConversationTimelineEntry }) {
  if (entry.message) {
    const author = entry.message.author
    return (
      <article
        className={`message ${author.kind === 'execution_worker' ? 'worker-message' : ''}`}
      >
        <header>
          <span className="avatar" aria-hidden="true">
            {author.displayName.charAt(0).toLocaleUpperCase()}
          </span>
          <strong>{author.displayName}</strong>
          <span className="identity-kind">
            {author.kind === 'external_agent' ? 'agent' : `@${author.handle}`}
          </span>
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
      </article>
    )
  }
  return <SystemTimelineEntry createdAt={entry.createdAt} />
}

/** One line per agent asked about a message, until they reply. */
function ReplyStatus({
  onRetry,
  requests,
}: {
  onRetry: (requestId: string) => void
  requests: ReplyRequest[]
}) {
  const visible = requests.filter((request) => request.status !== 'replied')
  if (!visible.length) return null
  return (
    <ul aria-label="Replies" className="reply-status">
      {visible.map((request) => (
        <li data-status={request.status} key={request.id}>
          <span aria-hidden="true" className="reply-status-dot" />
          <span>{describeRequest(request)}</span>
          {request.status === 'expired' || request.status === 'released' ? (
            <button
              className="link-button"
              onClick={() => onRetry(request.id)}
              type="button"
            >
              Ask again
            </button>
          ) : null}
        </li>
      ))}
    </ul>
  )
}

function describeRequest(request: ReplyRequest) {
  const name = request.agent.displayName
  switch (request.status) {
    case 'claimed':
      return `${name} is replying…`
    case 'pending':
      return request.agentLastSeenAt
        ? `Waiting for ${name} (last checked in ${timeAgo(request.agentLastSeenAt)})`
        : `Waiting for ${name} to connect`
    case 'passed':
      return `${name} had nothing to add`
    case 'released':
      return `${name} couldn't answer${request.reason ? `: ${request.reason}` : ''}`
    case 'expired':
      return `${name} stopped before replying`
    default:
      return name
  }
}

function timeAgo(iso: string) {
  const seconds = Math.max(0, (Date.now() - Date.parse(iso)) / 1000)
  if (!Number.isFinite(seconds)) return 'a while ago'
  if (seconds < 60) return 'just now'
  if (seconds < 3600) return `${Math.round(seconds / 60)}m ago`
  if (seconds < 86_400) return `${Math.round(seconds / 3600)}h ago`
  return `${Math.round(seconds / 86_400)}d ago`
}

function GroupMembers({
  conversationId,
  members,
  people,
  viewerId,
}: {
  conversationId: string
  members: ConversationActor[]
  people: ConversationActor[]
  viewerId: string
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [adding, setAdding] = useState('')
  const [error, setError] = useState<string | null>(null)
  const memberIds = new Set(members.map((member) => member.id))
  const candidates = people.filter((person) => !memberIds.has(person.id))
  const refresh = () => queryClient.invalidateQueries({ queryKey: directQueryKey })
  const run = (action: Promise<void>, after?: () => void) => {
    setError(null)
    void action
      .then(async () => {
        await refresh()
        after?.()
      })
      .catch((reason: unknown) =>
        setError(reason instanceof Error ? reason.message : 'That did not work.'),
      )
  }
  return (
    <aside aria-label="Group members" className="context-drawer">
      <h2>Members</h2>
      <ul className="member-list">
        {members.map((member) => (
          <li key={member.id}>
            <span>
              {member.displayName}
              {member.id === viewerId ? ' (you)' : ''}
            </span>
            <button
              className="secondary-button"
              onClick={() =>
                run(
                  removeGroupMember(conversationId, member.id),
                  member.id === viewerId ? () => navigate('/messages') : undefined,
                )
              }
              type="button"
            >
              {member.id === viewerId ? 'Leave' : 'Remove'}
            </button>
          </li>
        ))}
      </ul>
      {candidates.length > 0 ? (
        <form
          onSubmit={(event) => {
            event.preventDefault()
            if (adding) run(addGroupMember(conversationId, adding), () => setAdding(''))
          }}
        >
          <label htmlFor="group-add-member">Add someone</label>
          <select
            id="group-add-member"
            onChange={(event) => setAdding(event.target.value)}
            value={adding}
          >
            <option value="">Choose…</option>
            {candidates.map((person) => (
              <option key={person.id} value={person.id}>
                {person.displayName}
              </option>
            ))}
          </select>
          <button disabled={!adding} type="submit">
            Add
          </button>
        </form>
      ) : null}
      {error ? <p role="alert">{error}</p> : null}
      <p>
        <Link to="/messages">All messages</Link>
      </p>
    </aside>
  )
}

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
import { useProject } from '../projects/hooks'
import { NavigationSyncAlert } from '../projects/NavigationSyncAlert'
import {
  ProjectPageError,
  ProjectPageLoading,
} from '../projects/ProjectPageStates'
import { useRememberProjectLocation } from '../projects/useRememberProjectLocation'
import { AppShell } from '../shell/AppShell'
import { StructuredComposer } from '../conversations/StructuredComposer'
import type { ComposerSubmission } from '../conversations/composerModel'
import { ConversationHeader } from '../conversations/ConversationHeader'
import { ThreadStatusButton } from '../conversations/threadBoard'
import { useProjectConversationSidebar } from '../conversations/hooks'
import { useMessageActions } from '../conversations/MessageActions'
import { useConversationTools } from '../later/useConversationTools'
import { ReplyStatusList } from '../conversations/ReplyStatus'
import { groupReplyRequests } from '../conversations/replyStatusModel'
import { ConversationTimeline, OlderMessagesButton, PendingMessageRow, TimelineEntries } from '../conversations/Timeline'
import { useAutoMarkSeen } from '../conversations/useAutoMarkSeen'
import { useArchiveConversation } from '../conversations/useArchiveConversation'
import { SessionLinks } from '../launch/SessionLinks'
import { useLiveOlderEntries } from '../conversations/useLiveOlderEntries'
import { usePageTitle } from '../shell/documentTitleModel'
import {
  retryReplyRequest,
  searchMentionableActors,
  sendConversationMessage,
  stopReplyRequest,
} from '../conversations/api'
import type { ConversationTimelineEntry } from '../conversations/types'
import { useConversationRealtime } from '../realtime/realtimeContext'
import { useSetWorkItemThreadLink } from '../work-items/hooks'
import { searchWorkItemReferences } from '../work-items/api'
import {
  ActorTag,
  Avatar,
  Banner,
  Button,
  ConversationIntro,
  Dialog,
  Dots,
  ErrorState,
  IconButton,
  MessageSkeleton,
  SegmentedControl,
} from '../ui'
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
  const workItemLink = useSetWorkItemThreadLink()
  const realtime = useConversationRealtime(threadId)
  const timeline = thread.timeline.data
  const verifiedTimeline = thread.routeMatches ? timeline : undefined
  usePageTitle(verifiedTimeline?.conversation.name ?? null)
  const activeRun = verifiedTimeline?.activeRun ?? null
  const latestTerminalRun = verifiedTimeline?.latestTerminalRun ?? null
  const [detailsOpen, setDetailsOpen] = useState(false)
  const [pendingSends, setPendingSends] = useState<{ key: string; text?: string }[]>([])
  const [archiveOpen, setArchiveOpen] = useState(false)
  const [composerMode, setComposerMode] = useState<'message' | 'run'>('message')
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
        throw new Error('Older timeline page crossed its Project boundary.')
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
  const retryAgent = (requestId: string) => void retryReplyRequest(requestId).then(() => thread.timeline.refetch())
  const stopAgent = verifiedTimeline?.capabilities.sendMessage
    ? (requestId: string) => stopReplyRequest(requestId).then(() => thread.timeline.refetch())
    : undefined
  const olderCursor = older.cursor

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

  const searchActors = async (query: string) => (await searchMentionableActors(projectId, query)).actors
  const searchWorkItems = (query: string) => searchWorkItemReferences(projectId, query)
  const messageActions = useMessageActions({
    projectId,
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
  })

  const sendMessage = async (submission: ComposerSubmission) => {
    setPendingSends((current) => [...current, { key: submission.idempotencyKey, text: submission.text }])
    try {
      await sendConversationMessage({
        conversationId: threadId,
        kind: 'thread',
        ...submission,
      })
      await thread.timeline.refetch()
    } finally {
      setPendingSends((current) => current.filter((item) => item.key !== submission.idempotencyKey))
    }
  }

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
        <ProjectPageError onRetry={() => void project.refetch()} />
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
        threadId={threadId}
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
              <ThreadStatusButton
                onSaved={() => void thread.timeline.refetch()}
                thread={{
                  id: threadId,
                  name: threadName,
                  status: verifiedTimeline.conversation.threadStatus ?? 'Open',
                  outcome: verifiedTimeline.conversation.threadOutcome,
                }}
              />
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
                { separator: true },
                ...tools.levelMenu,
                ...(verifiedTimeline.capabilities.archive && !archived
                  ? [{ separator: true as const }, { label: 'Archive thread', icon: 'archive' as const, danger: true, onSelect: () => setArchiveOpen(true) }]
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
      {archived ? (
        <Banner className="aw-conv__banner" icon="archive" title="This thread is archived." tone="offline">
          You can read it, but no one can post or run work here.
        </Banner>
      ) : null}

      <ConversationTimeline
        busy={thread.timeline.isPending}
        label={`Messages in ${threadName}`}
        revision={`${verifiedTimeline?.highestSequence ?? 0}:${pendingSends.length}:${Object.values(realtime.streaming).join('').length}:${pendingApproval?.id ?? ''}:${runForCard?.state ?? ''}`}
      >
        {olderCursor ? <OlderMessagesButton busy={older.loading} onClick={() => void older.loadOlder()} /> : null}
        {older.error ? <Banner tone="danger">{older.error}</Banner> : null}
        {thread.timeline.isPending ? <MessageSkeleton label="Loading thread" /> : null}
        {thread.timeline.isError ? (
          <ErrorState action={<Button onClick={() => void thread.timeline.refetch()}>Try again</Button>} title="This thread didn’t load">
            It may have been removed from this Project, or it couldn’t be reached.
          </ErrorState>
        ) : null}
        {verifiedTimeline && !olderCursor ? (
          <ConversationIntro icon="messages" title={threadName}>
            {verifiedTimeline.capabilities.workerInvoke
              ? 'A focused conversation. Ask an agent to make a change here and it runs in its own workspace, with every step, approval and result kept in this thread.'
              : 'A focused conversation inside the channel. Everything said here stays with this thread.'}
          </ConversationIntro>
        ) : null}
        <TimelineEntries
          agentName={(runId) => runAgent([activeRun, latestTerminalRun].find((run) => run?.id === runId)).name}
          entries={entries}
          onOpenHistory={messageActions.openHistory}
          renderEditor={messageActions.editorFor}
          renderExtras={(entry) =>
            entry.message && !entry.message.removed ? (
              <ReplyStatusList onRetry={retryAgent} onStop={stopAgent} requests={requestsByMessage.get(entry.message.id) ?? []} />
            ) : null
          }
          renderReactions={messageActions.reactionsFor}
          renderToolbar={messageActions.toolbarFor}
          viewerId={session.user.actorId}
        />
        {trailingRequests.length ? (
          <div className="aw-replies--trailing">
            <ReplyStatusList onRetry={retryAgent} onStop={stopAgent} requests={trailingRequests} />
          </div>
        ) : null}
        {pendingSends.map((item, index) => (
          <PendingMessageRow
            continued={index > 0 || entries.at(-1)?.message?.author.id === session.user.actorId}
            key={item.key}
            name={session.user.displayName}
            seed={session.user.actorId}
            text={item.text}
          />
        ))}
        {Object.entries(realtime.streaming).map(([messageId, text]) => (
          <article aria-label={`${writer.name} is writing`} className="aw-msg is-streaming" key={messageId}>
            <div className="aw-msg__gutter">
              <Avatar kind="worker" name={writer.name} seed={writer.seed} size={36} working />
            </div>
            <div className="aw-msg__main">
              <div className="aw-msg__meta">
                <span className="aw-msg__author">{writer.name}</span>
                <ActorTag kind="worker" />
                <Dots label="Writing" />
              </div>
              <div className="aw-msg__body">{text}</div>
            </div>
          </article>
        ))}
        {runForCard ? (
          <div className="aw-timeline__card">
            <RunCard actions={cardActions} notes={cardNotes} run={runForCard} />
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
          label={`Message ${threadName}`}
          onSubmit={sendMessage}
          placeholder={`Reply in ${threadName}`}
          searchActors={searchActors}
          searchWorkItems={searchWorkItems}
          toolbar={modeSwitch}
          {...tools.composer}
        />
      ) : null}
      {mode === 'run' && showRunComposer ? <RunComposer actions={runActions} activeRun={activeRun} modeSwitch={modeSwitch} /> : null}
      {verifiedTimeline && archived ? (
        <StructuredComposer disabledReason="This thread is archived. Nobody can post here." onSubmit={async () => undefined} searchActors={async () => []} />
      ) : null}

      {messageActions.dialogs}
      <Dialog
        description="People can still read it from Archived conversations, but no one can post or start runs."
        footer={
          <>
            <Button onClick={() => setArchiveOpen(false)}>Cancel</Button>
            <Button
              busy={archiveThread.isPending}
              onClick={() => {
                if (!verifiedTimeline) return
                archiveThread.mutate(verifiedTimeline.conversation.version ?? 1, { onSettled: () => setArchiveOpen(false) })
              }}
              variant="danger"
            >
              Archive thread
            </Button>
          </>
        }
        onClose={() => setArchiveOpen(false)}
        open={archiveOpen}
        title={`Archive “${threadName}”?`}
      />
    </AppShell>
  )
}

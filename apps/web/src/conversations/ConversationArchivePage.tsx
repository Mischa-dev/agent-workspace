import { useQuery } from '@tanstack/react-query'
import { Link, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import { Button, EmptyState, ErrorState, Icon, ListSkeleton, PageHeader, Section, formatFullDate, formatShortDate } from '../ui'
import { getConversationTimeline } from './api'
import { useProjectConversationSidebar } from './hooks'
import { useRestoreConversation } from './useArchiveConversation'
import './chat.css'
import '../styles/pages.css'

/** "Archived Oct 12", with the full date on hover. */
function ArchivedOn({ at }: { at: string | null }) {
  if (!at) return null
  return (
    <span className="aw-archive__meta" title={formatFullDate(at)}>
      Archived {formatShortDate(at)}
    </span>
  )
}

/**
 * Restore, for a row whose conversation you may bring back. Asks the
 * conversation itself, which knows its version and whether you can.
 */
function RestoreButton({ conversationId, kind, label, projectId }: { conversationId: string; kind: 'channel' | 'thread'; label: string; projectId: string }) {
  const facts = useQuery({
    queryKey: ['project', projectId, 'conversation', conversationId, 'restorable'],
    queryFn: () => getConversationTimeline(conversationId, undefined, 1, true),
    staleTime: 30_000,
    retry: false,
  })
  const restore = useRestoreConversation({ conversationId, kind, label, projectId })
  if (!facts.data?.capabilities.restore) return null
  return (
    <Button
      aria-label={`Restore ${label}`}
      busy={restore.isPending}
      icon="undo"
      onClick={() => restore.mutate(facts.data.conversation.version)}
      size="sm"
      variant="ghost"
    >
      Restore
    </Button>
  )
}

export function ConversationArchivePage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const conversations = useProjectConversationSidebar(projectId, '', {
    includeArchived: true,
  })

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

  const archivedChannels = conversations.data?.channels.filter((channel) => channel.archivedAt)
  const archivedThreads = conversations.data?.channels.flatMap((channel) =>
    channel.threads
      .filter((thread) => thread.archivedAt || thread.parentArchivedAt)
      .map((thread) => ({ ...thread, channelName: channel.name })),
  )
  const empty = conversations.isSuccess && !archivedChannels?.length && !archivedThreads?.length

  const base = `/projects/${encodeURIComponent(projectId)}`
  return (
    <AppShell mainClassName="aw-main--page" project={project.data} user={session.user}>
      <div className="aw-page aw-page--reading">
        <PageHeader icon="archive" title="Archived conversations" />
        {conversations.isPending ? <ListSkeleton label="Loading archived conversations" rows={4} /> : null}
        {conversations.isError ? (
          <ErrorState error={conversations.error} onRetry={() => void conversations.refetch()} title="The archive didn’t load" />
        ) : null}
        {empty ? (
          <EmptyState icon="archive" title="Nothing archived">
            When you archive a channel or thread, it moves here and stays readable. You can restore it from here.
          </EmptyState>
        ) : null}
        {archivedChannels?.length ? (
          <Section title="Channels">
            <ul className="aw-archive">
              {archivedChannels.map((channel) => (
                <li className="aw-archive__item" key={channel.id}>
                  <Link className="aw-archive__row" to={`${base}/channels/${encodeURIComponent(channel.id)}?includeArchived=true`}>
                    <Icon name="hash" />
                    <span className="aw-archive__name">{channel.name}</span>
                    <ArchivedOn at={channel.archivedAt} />
                    <Icon className="aw-archive__chev" name="chevron-right" />
                  </Link>
                  <RestoreButton conversationId={channel.id} kind="channel" label={`#${channel.name}`} projectId={projectId} />
                </li>
              ))}
            </ul>
          </Section>
        ) : null}
        {archivedThreads?.length ? (
          <Section title="Threads">
            <ul className="aw-archive">
              {archivedThreads.map((thread) => (
                <li className="aw-archive__item" key={thread.id}>
                  <Link className="aw-archive__row" to={`${base}/threads/${encodeURIComponent(thread.id)}?includeArchived=true`}>
                    <Icon name="messages" />
                    <span className="aw-archive__name">{thread.name}</span>
                    <span className="aw-archive__meta">in #{thread.channelName}</span>
                    <ArchivedOn at={thread.archivedAt} />
                    <Icon className="aw-archive__chev" name="chevron-right" />
                  </Link>
                  {/* A thread under an archived channel comes back with its channel. */}
                  {thread.archivedAt && !thread.parentArchivedAt ? (
                    <RestoreButton conversationId={thread.id} kind="thread" label={`“${thread.name}”`} projectId={projectId} />
                  ) : null}
                </li>
              ))}
            </ul>
          </Section>
        ) : null}
      </div>
    </AppShell>
  )
}

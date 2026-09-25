import { Link, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import {
  ProjectPageError,
  ProjectPageLoading,
} from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import { Button, EmptyState, ErrorState, Icon, MessageSkeleton, PageHeader, Section } from '../ui'
import { useProjectConversationSidebar } from './hooks'

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
        <ProjectPageError onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  const archivedChannels = conversations.data?.channels.filter(
    (channel) => channel.archivedAt,
  )
  const archivedThreads = conversations.data?.channels.flatMap((channel) =>
    channel.threads
      .filter((thread) => thread.archivedAt || thread.parentArchivedAt)
      .map((thread) => ({ ...thread, channelName: channel.name })),
  )
  const empty =
    conversations.isSuccess &&
    !archivedChannels?.length &&
    !archivedThreads?.length

  const base = `/projects/${encodeURIComponent(projectId)}`
  return (
    <AppShell mainClassName="aw-main--page" project={project.data} user={session.user}>
      <div className="aw-page aw-page--narrow">
        <PageHeader icon="archive" title="Archived conversations">
          Archived channels and threads stay readable here. Nobody can post in them.
        </PageHeader>
        {conversations.isPending ? <MessageSkeleton label="Loading archived conversations" rows={2} /> : null}
        {conversations.isError ? (
          <ErrorState action={<Button onClick={() => void conversations.refetch()}>Try again</Button>} title="The archive didn’t load">
            Check your connection and try again.
          </ErrorState>
        ) : null}
        {empty ? (
          <EmptyState icon="archive" title="Nothing archived">
            When you archive a channel or thread, it moves here and stays readable.
          </EmptyState>
        ) : null}
        {archivedChannels?.length ? (
          <Section title="Channels">
            <ul className="aw-archive">
              {archivedChannels.map((channel) => (
                <li key={channel.id}>
                  <Link className="aw-archive__row" to={`${base}/channels/${encodeURIComponent(channel.id)}?includeArchived=true`}>
                    <Icon name="hash" />
                    <span className="aw-archive__name">{channel.name}</span>
                    <Icon className="aw-archive__chev" name="chevron-right" />
                  </Link>
                </li>
              ))}
            </ul>
          </Section>
        ) : null}
        {archivedThreads?.length ? (
          <Section title="Threads">
            <ul className="aw-archive">
              {archivedThreads.map((thread) => (
                <li key={thread.id}>
                  <Link className="aw-archive__row" to={`${base}/threads/${encodeURIComponent(thread.id)}?includeArchived=true`}>
                    <Icon name="messages" />
                    <span className="aw-archive__name">{thread.name}</span>
                    <span className="aw-archive__meta">in #{thread.channelName}</span>
                    <Icon className="aw-archive__chev" name="chevron-right" />
                  </Link>
                </li>
              ))}
            </ul>
          </Section>
        ) : null}
      </div>
    </AppShell>
  )
}

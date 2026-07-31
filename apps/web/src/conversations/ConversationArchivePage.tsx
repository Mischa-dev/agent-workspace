import { Link, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import {
  ProjectPageError,
  ProjectPageLoading,
} from '../projects/ProjectOverviewPage'
import { useProject } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
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

  return (
    <AppShell project={project.data} user={session.user}>
      <div className="page-content">
        <header className="page-heading">
          <div>
            <span className="section-label">Conversation history</span>
            <h1>Archived Conversations</h1>
            <p>Read-only Channels and Threads remain available as durable history.</p>
          </div>
        </header>
        {conversations.isPending ? (
          <p aria-busy="true" role="status">
            Loading archived Conversations…
          </p>
        ) : null}
        {conversations.isError ? (
          <div className="error-state" role="alert">
            <h2>Archive unavailable</h2>
            <button onClick={() => void conversations.refetch()} type="button">
              Try again
            </button>
          </div>
        ) : null}
        {empty ? <p className="notice">No Conversations are archived.</p> : null}
        {archivedChannels?.length ? (
          <section aria-labelledby="archived-channels-heading" className="page-section">
            <h2 id="archived-channels-heading">Channels</h2>
            <ul className="archive-list">
              {archivedChannels.map((channel) => (
                <li key={channel.id}>
                  <Link
                    to={`/projects/${encodeURIComponent(projectId)}/channels/${encodeURIComponent(channel.id)}?includeArchived=true`}
                  >
                    <span aria-hidden="true">#</span>
                    {channel.name}
                  </Link>
                </li>
              ))}
            </ul>
          </section>
        ) : null}
        {archivedThreads?.length ? (
          <section aria-labelledby="archived-threads-heading" className="page-section">
            <h2 id="archived-threads-heading">Threads</h2>
            <ul className="archive-list">
              {archivedThreads.map((thread) => (
                <li key={thread.id}>
                  <Link
                    to={`/projects/${encodeURIComponent(projectId)}/threads/${encodeURIComponent(thread.id)}?includeArchived=true`}
                  >
                    <span aria-hidden="true">↳</span>
                    <span>
                      {thread.name} <small>in {thread.channelName}</small>
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          </section>
        ) : null}
      </div>
    </AppShell>
  )
}

import { Link, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { useProjectConversationSidebar } from '../conversations/hooks'
import type { SidebarChannel, SidebarThread } from '../conversations/types'
import { AppShell } from '../shell/AppShell'
import { Avatar, Banner, Button, Dots, EmptyState, Icon, MessageSkeleton, PageHeader, Row, RowList, Section, actorKind } from '../ui'
import { useWorkItems } from '../work-items/hooks'
import { StatusIcon } from '../work-items/WorkItemIcons'
import { roleLabel } from './format'
import { NavigationSyncAlert } from './NavigationSyncAlert'
import { ProjectPageError, ProjectPageLoading } from './ProjectPageStates'
import { ProjectInitials } from './ProjectSwitcher'
import { useProject } from './hooks'
import type { Project } from './types'
import { useRememberProjectLocation } from './useRememberProjectLocation'

const RECENT_WORK = { limit: 5 }

export function ProjectOverviewPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'project_overview' },
    project.isSuccess && !project.data.archivedAt && project.data.capabilities.navigationWrite,
    project.data?.navigation,
  )

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

  const base = `/projects/${encodeURIComponent(project.data.id)}`
  return (
    <AppShell project={project.data} user={session.user}>
      <div className="aw-page">
        <NavigationSyncAlert {...navigationSync} />
        <PageHeader
          actions={
            project.data.capabilities.manage && !project.data.archivedAt ? (
              <Link className="aw-btn aw-btn--secondary aw-btn--md" to={`${base}/settings`}>
                <Icon name="settings" />
                <span className="aw-btn__label">Project settings</span>
              </Link>
            ) : null
          }
          mark={<ProjectInitials project={project.data} size={40} />}
          title={project.data.name}
        >
          {project.data.key} · {roleLabel(project.data.role)}
          {project.data.attentionCount
            ? ` · ${project.data.attentionCount} ${project.data.attentionCount === 1 ? 'thing needs' : 'things need'} you`
            : ''}
        </PageHeader>
        {project.data.archivedAt && (
          <Banner className="aw-page__banner" icon="archive" title="This Project is archived.">
            You can read everything in it, but nothing new can be added.
          </Banner>
        )}
        <div className="aw-overview">
          <Conversations base={base} project={project.data} />
          <RecentWork base={base} projectId={project.data.id} />
        </div>
      </div>
    </AppShell>
  )
}

const WAITING: SidebarThread['attention'][] = ['response_required', 'failed']

function channelMeta(channel: SidebarChannel) {
  if (channel.unread) return 'New messages'
  const count = channel.threads.filter((thread) => !thread.archivedAt).length
  if (!count) return channel.hasMoreThreads ? undefined : 'No threads yet'
  return `${count}${channel.hasMoreThreads ? '+' : ''} ${count === 1 && !channel.hasMoreThreads ? 'thread' : 'threads'}`
}

function Conversations({ base, project }: { base: string; project: Project }) {
  const sidebar = useProjectConversationSidebar(project.id)
  if (sidebar.isPending) {
    return (
      <Section title="Needs you">
        <MessageSkeleton label="Loading conversations" rows={2} />
      </Section>
    )
  }
  if (sidebar.isError) {
    return (
      <Section title="Needs you">
        <Banner action={<Button onClick={() => void sidebar.refetch()} size="sm" variant="ghost">Try again</Button>} tone="danger">
          Conversations didn’t load.
        </Banner>
      </Section>
    )
  }
  const channels = sidebar.data.channels.filter((channel) => !channel.archivedAt)
  const threads = channels.flatMap((channel) =>
    channel.threads.filter((thread) => !thread.archivedAt).map((thread) => ({ ...thread, channelName: channel.name })),
  )
  const waiting = threads.filter((thread) => WAITING.includes(thread.attention))
  const working = threads.filter((thread) => thread.attention === 'active')

  return (
    <div className="aw-overview__column">
      <Section description="Threads waiting on an answer or a decision from you." title="Needs you">
        {waiting.length ? (
          <RowList label="Threads that need you">
            {waiting.map((thread) => (
              <ThreadRow base={base} key={thread.id} thread={thread} />
            ))}
          </RowList>
        ) : (
          <p className="aw-quiet">Nothing is waiting on you here.</p>
        )}
      </Section>
      {working.length > 0 && (
        <Section title="Working now">
          <RowList label="Threads with work running">
            {working.map((thread) => (
              <ThreadRow base={base} key={thread.id} thread={thread} />
            ))}
          </RowList>
        </Section>
      )}
      <Section title="Channels">
        {channels.length ? (
          <RowList label="Channels">
            {channels.map((channel) => (
              <Row
                actions={<Icon className="aw-row__chev" name="chevron-right" />}
                className={channel.unread ? 'aw-row--link is-unread' : 'aw-row--link'}
                key={channel.id}
                leading={<Icon className="aw-row__glyph" name="hash" />}
                meta={channelMeta(channel)}
                title={
                  <Link className="aw-row__link" to={`${base}/channels/${encodeURIComponent(channel.id)}`}>
                    {channel.name}
                  </Link>
                }
              />
            ))}
          </RowList>
        ) : (
          <p className="aw-quiet">No channels yet. Start one from the Project menu in the sidebar.</p>
        )}
      </Section>
    </div>
  )
}

function ThreadRow({ base, thread }: { base: string; thread: SidebarThread & { channelName: string } }) {
  return (
    <Row
      actions={<Icon className="aw-row__chev" name="chevron-right" />}
      className="aw-row--link"
      leading={
        <span className={`aw-row__tile aw-row__tile--${thread.attention}`}>
          {thread.attention === 'active' ? (
            <Dots />
          ) : (
            <Icon name={thread.attention === 'failed' ? 'triangle-alert' : 'messages'} size={16} />
          )}
        </span>
      }
      meta={`#${thread.channelName} · ${thread.attentionLabel}`}
      title={
        <Link className="aw-row__link" to={`${base}/threads/${encodeURIComponent(thread.id)}`}>
          {thread.name}
        </Link>
      }
    />
  )
}

function RecentWork({ base, projectId }: { base: string; projectId: string }) {
  const work = useWorkItems(projectId, RECENT_WORK)
  const viewAll = (
    <Link className="aw-textlink" to={`${base}/work-items`}>
      View all
    </Link>
  )
  return (
    <div className="aw-overview__column">
      <Section actions={viewAll} title="Recent work items">
        {work.isPending && <MessageSkeleton label="Loading work items" rows={3} />}
        {work.isError && (
          <Banner action={<Button onClick={() => void work.refetch()} size="sm" variant="ghost">Try again</Button>} tone="danger">
            Work items didn’t load.
          </Banner>
        )}
        {work.isSuccess && !work.data.items.length && (
          <EmptyState icon="work" title="No work items yet">
            Track what needs doing here, then hand it to a person or an agent.
          </EmptyState>
        )}
        {work.isSuccess && work.data.items.length > 0 && (
          <RowList label="Recent work items">
            {work.data.items.map((item) => (
              <Row
                actions={
                  item.assignee ? (
                    <Avatar kind={actorKind(item.assignee.kind)} name={item.assignee.displayName} seed={item.assignee.actorId} size={20} />
                  ) : null
                }
                className="aw-row--link"
                key={item.id}
                leading={<StatusIcon category={item.status.semanticCategory} label={item.status.name} />}
                meta={`${item.key} · ${item.status.name}`}
                title={
                  <Link className="aw-row__link" to={`${base}/work-items/${encodeURIComponent(item.id)}`}>
                    {item.title}
                  </Link>
                }
              />
            ))}
          </RowList>
        )}
      </Section>
    </div>
  )
}

import { Suspense, lazy, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'

import { requestJson } from '../api/client'
import { toastError } from '../app/mutations'
import { useAuthenticatedSession } from '../auth/session'
import { useProjectExperience, type Experience } from '../experience/layout'
import { useProjectConversationSidebar } from '../conversations/hooks'
import type { SidebarChannel, SidebarThread } from '../conversations/types'
import { inboxKeys, isExternalHref, normalizePage, useUpdateInbox, type InboxNotification } from '../inbox/api'
import { FAILURE_KINDS, KIND_ICON } from '../inbox/kinds'
import { projectSettingsHref } from '../settings/paths'
import { AppShell } from '../shell/AppShell'
import {
  Avatar,
  Banner,
  Button,
  ButtonLink,
  Dots,
  EmptyState,
  Icon,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  actorKind,
  countLabel,
  formatFullDate,
  formatRelative,
  showToast,
} from '../ui'
import { useWorkItems } from '../work-items/hooks'
import { StatusIcon } from '../work-items/WorkItemIcons'
import { roleLabel } from './format'
import { useProject, useRestoreProject } from './hooks'
import { MoreLink } from './MoreLink'
import { NavigationSyncAlert } from './NavigationSyncAlert'
import './overview.css'
import { ProjectInitials } from './ProjectInitials'
import { ProjectPageError, ProjectPageLoading } from './ProjectPageStates'
import type { Project } from './types'
import { useRememberProjectLocation } from './useRememberProjectLocation'

// The project's whiteboards and the New channel dialog load in their own small chunks.
const ProjectWhiteboards = lazy(() => import('./ProjectWhiteboards'))
const ChannelCreator = lazy(() => import('../shell/ChannelCreator'))

const RECENT_WORK = { limit: 5 }
const SHOWN = 5

export function ProjectOverviewPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const experience = useProjectExperience(projectId)
  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'project_overview' },
    project.isSuccess && !project.data.archivedAt && project.data.capabilities.navigationWrite,
    project.data?.navigation,
  )

  // While it loads, the shell finds the project from the address, so its sidebar stays put.
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

  const base = `/projects/${encodeURIComponent(project.data.id)}`
  return (
    <AppShell project={project.data} user={session.user}>
      <div className="aw-page">
        <NavigationSyncAlert {...navigationSync} />
        <PageHeader
          actions={
            project.data.capabilities.manage && !project.data.archivedAt ? (
              <ButtonLink icon="settings" to={projectSettingsHref(project.data.id)}>
                Project settings
              </ButtonLink>
            ) : null
          }
          mark={<ProjectInitials project={project.data} size={40} />}
          title={project.data.name}
        >
          {experience.shows('tech.task_keys') ? `${project.data.key} · ` : ''}
          {roleLabel(project.data.role)}
        </PageHeader>
        {project.data.archivedAt && <ArchivedBanner project={project.data} />}
        <div className="aw-overview">
          <div className="aw-overview__column">
            <NeedsYouHere base={base} project={project.data} />
            {experience.shows('area.chat') ? <Channels base={base} project={project.data} /> : null}
          </div>
          {experience.shows('area.tasks') ? (
            <RecentTasks
              base={base}
              canCreate={project.data.role !== 'viewer' && !project.data.archivedAt}
              experience={experience}
              projectId={project.data.id}
            />
          ) : null}
        </div>
        {experience.shows('area.whiteboards') ? (
          <Suspense fallback={null}>
            <ProjectWhiteboards canEdit={project.data.role !== 'viewer' && !project.data.archivedAt} projectId={project.data.id} />
          </Suspense>
        ) : null}
      </div>
    </AppShell>
  )
}

const WAITING: SidebarThread['attention'][] = ['response_required', 'failed']

type ThreadInChannel = SidebarThread & { channelName: string }

function liveThreads(channels: SidebarChannel[]): ThreadInChannel[] {
  return channels.flatMap((channel) =>
    channel.threads.filter((thread) => !thread.archivedAt).map((thread) => ({ ...thread, channelName: channel.name })),
  )
}

/** The page a notification opens, without the message it points at. */
function pathOf(href: string) {
  return href.split(/[?#]/)[0]
}

/** This project's unread inbox items, under the inbox's key so reading one anywhere refreshes it. */
function useProjectUnread(projectId: string) {
  return useQuery({
    queryKey: [...inboxKeys.root, 'project', projectId],
    queryFn: async () => {
      const search = new URLSearchParams({ view: 'unread', category: 'all', projectId, limit: '20' })
      return normalizePage(await requestJson<unknown>(`/api/v1/inbox?${search}`))
    },
  })
}

/**
 * What needs you in this project, from the same places the header, the rail
 * badge and the Inbox count: unread inbox items here, then threads waiting on
 * an answer that the inbox doesn't already list.
 */
/** Says an archived project is read-only, with a way back for whoever manages it. */
function ArchivedBanner({ project }: { project: Project }) {
  const restore = useRestoreProject()
  return (
    <Banner
      action={
        project.capabilities.manage ? (
          <Button
            busy={restore.isPending}
            icon="undo"
            onClick={() =>
              restore.mutate(
                { projectId: project.id, expectedVersion: project.version },
                {
                  onSuccess: () => showToast({ tone: 'success', title: `${project.name} is back` }),
                  onError: (error) => toastError(`${project.name} wasn’t restored`, error),
                },
              )
            }
            size="sm"
          >
            Restore
          </Button>
        ) : null
      }
      className="aw-page__banner"
      icon="archive"
      title="This project is archived."
    >
      You can read everything in it, but nothing new can be added.
    </Banner>
  )
}

function NeedsYouHere({ base, project }: { base: string; project: Project }) {
  const unread = useProjectUnread(project.id)
  const sidebar = useProjectConversationSidebar(project.id)
  const update = useUpdateInbox('unread', 'all')
  if (unread.isPending || sidebar.isPending) {
    return (
      <Section title="Needs you">
        <ListSkeleton label="Loading what needs you" rows={2} />
      </Section>
    )
  }
  const notifications = unread.data?.notifications ?? []
  const listed = new Set(notifications.map((item) => pathOf(item.href)))
  const waiting = liveThreads((sidebar.data?.channels ?? []).filter((channel) => !channel.archivedAt)).filter(
    (thread) => WAITING.includes(thread.attention) && !listed.has(`${base}/threads/${encodeURIComponent(thread.id)}`),
  )
  // The list is the whole count when it fits on one page; past that, the project's count says how many.
  const unreadTotal = unread.data?.nextCursor ? Math.max(project.attentionCount, notifications.length) : notifications.length
  const total = unreadTotal + waiting.length
  const shownNotifications = notifications.slice(0, SHOWN)
  const shownThreads = waiting.slice(0, Math.max(0, SHOWN - shownNotifications.length))
  const hidden = total - shownNotifications.length - shownThreads.length
  const markRead = (item: InboxNotification) => {
    if (!item.read) update.mutate({ action: 'read', ids: [item.id] })
  }
  return (
    <Section actions={<MoreLink to="/inbox">Inbox</MoreLink>} id="overview-needs-you" title="Needs you">
      {unread.isError || sidebar.isError ? (
        <Banner
          onRetry={() => {
            if (unread.isError) void unread.refetch()
            if (sidebar.isError) void sidebar.refetch()
          }}
          tone="danger"
        >
          {unread.isError ? 'The inbox didn’t load.' : 'Conversations didn’t load.'}
        </Banner>
      ) : null}
      {total === 0 && !unread.isError && !sidebar.isError ? <p className="aw-quiet">Nothing here needs you.</p> : null}
      {shownNotifications.length + shownThreads.length > 0 ? (
        <RowList label="Needs you">
          {shownNotifications.map((item) => (
            <Row
              className="aw-row--link"
              key={item.id}
              leading={
                <span className={FAILURE_KINDS.has(item.kind) ? 'aw-row__tile aw-row__tile--failed' : 'aw-row__tile'}>
                  <Icon name={KIND_ICON[item.kind] ?? 'inbox'} size={16} />
                </span>
              }
              meta={<span title={formatFullDate(item.at)}>{formatRelative(item.at)}</span>}
              title={
                isExternalHref(item.href) ? (
                  <a className="aw-row__link" href={item.href} onClick={() => markRead(item)} rel="noopener noreferrer" target="_blank">
                    {item.title}
                  </a>
                ) : (
                  <Link className="aw-row__link" onClick={() => markRead(item)} to={item.href}>
                    {item.title}
                  </Link>
                )
              }
            />
          ))}
          {shownThreads.map((thread) => (
            <ThreadRow base={base} key={thread.id} thread={thread} />
          ))}
        </RowList>
      ) : null}
      {hidden > 0 ? <p className="aw-overview__more">And {hidden} more in the inbox.</p> : null}
    </Section>
  )
}

/** Every channel reads the same way: anything new first, then how many threads it has. */
function channelMeta(channel: SidebarChannel) {
  const count = channel.threads.filter((thread) => !thread.archivedAt).length
  const threads = count ? (channel.hasMoreThreads ? `${count}+ threads` : countLabel(count, 'thread')) : null
  return [channel.unread ? 'New messages' : null, threads].filter(Boolean).join(' · ') || 'Nothing new'
}

function Channels({ base, project }: { base: string; project: Project }) {
  const sidebar = useProjectConversationSidebar(project.id)
  const navigate = useNavigate()
  const [creating, setCreating] = useState(false)
  const canCreate = Boolean(sidebar.data?.capabilities.createChannel) && !project.archivedAt
  const channels = (sidebar.data?.channels ?? []).filter((channel) => !channel.archivedAt)
  const working = liveThreads(channels).filter((thread) => thread.attention === 'active')
  return (
    <>
      {working.length > 0 && (
        <Section title="Working now">
          <RowList label="Threads with work running">
            {working.map((thread) => (
              <ThreadRow base={base} key={thread.id} thread={thread} />
            ))}
          </RowList>
        </Section>
      )}
      <Section
        actions={
          canCreate && channels.length > 0 ? (
            <Button icon="plus" onClick={() => setCreating(true)} size="sm" variant="ghost">
              New channel
            </Button>
          ) : null
        }
        title="Channels"
      >
        {sidebar.isPending ? <ListSkeleton label="Loading channels" rows={3} /> : null}
        {sidebar.isError ? (
          <Banner onRetry={() => void sidebar.refetch()} tone="danger">
            Channels didn’t load.
          </Banner>
        ) : null}
        {sidebar.isSuccess && channels.length === 0 ? (
          <EmptyState
            action={
              canCreate ? (
                <Button icon="plus" onClick={() => setCreating(true)} variant="primary">
                  New channel
                </Button>
              ) : null
            }
            icon="hash"
            title="No channels yet"
          >
            Channels are where this project’s conversations happen.
          </EmptyState>
        ) : null}
        {channels.length > 0 ? (
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
        ) : null}
      </Section>
      {creating ? (
        <Suspense fallback={null}>
          <ChannelCreator onClose={() => setCreating(false)} onCreated={(to) => navigate(to)} open projectId={project.id} />
        </Suspense>
      ) : null}
    </>
  )
}

function ThreadRow({ base, thread }: { base: string; thread: ThreadInChannel }) {
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

function RecentTasks({ base, canCreate, experience, projectId }: { base: string; canCreate: boolean; experience: Experience; projectId: string }) {
  const work = useWorkItems(projectId, RECENT_WORK)
  const many = experience.word('task', 'many')
  return (
    <div className="aw-overview__column">
      <Section actions={<MoreLink to={`${base}/work-items`}>{`All ${many}`}</MoreLink>} title={`Recent ${many}`}>
        {work.isPending && <ListSkeleton label={`Loading ${many}`} rows={3} />}
        {work.isError && (
          <Banner onRetry={() => void work.refetch()} tone="danger">
            {experience.label('task', 'many')} didn’t load.
          </Banner>
        )}
        {work.isSuccess && !work.data.items.length && (
          <EmptyState
            action={
              canCreate ? (
                <ButtonLink icon="plus" to={`${base}/work-items?create=task`} variant="primary">
                  New {experience.word('task')}
                </ButtonLink>
              ) : null
            }
            icon="work"
            title={`No ${many} yet`}
          >
            Track what needs doing here, then hand it to a person or an agent.
          </EmptyState>
        )}
        {work.isSuccess && work.data.items.length > 0 && (
          <RowList label={`Recent ${many}`}>
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
                meta={experience.shows('tech.task_keys') ? `${item.key} · ${item.status.name}` : item.status.name}
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

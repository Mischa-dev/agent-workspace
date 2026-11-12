import { lazy, Suspense, useState, type ReactNode } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'

import { useProjectConversationSidebar } from '../conversations/hooks'
import type { SidebarThread } from '../conversations/types'
import { toastError } from '../app/toastError'
import { useProjectExperience } from '../experience/layout'
import type { PartId } from '../experience/parts'
import { useProjects, useSetRailOrder, useUpdateProjectNavigation } from '../projects/hooks'
import type { Project } from '../projects/types'
import { IconButton, showToast, type MenuEntry } from '../ui'
import { projectSection } from './projectSections'
import { railOrderAfterStep, railProjects } from './railProjects'
import { SidebarFrame, SidebarLink, SidebarSection, type SidebarAttention } from './SidebarParts'
import { projectSettingsHref } from '../settings/paths'

const PinnedViews = lazy(() => import('./PinnedViews'))
const SidebarAgents = lazy(() => import('../agents/SidebarAgents'))
const ChannelCreator = lazy(() => import('./ChannelCreator'))

const attentionMarks: Partial<Record<SidebarThread['attention'], SidebarAttention>> = {
  response_required: 'needs-you',
  active: 'working',
  failed: 'failed',
  completed: 'done',
}

export function ProjectSidebar({
  mobileOpen,
  onNavigate,
  project,
  selectedChannelId = '',
  thread,
}: {
  mobileOpen: boolean
  onNavigate: () => void
  project: Project
  selectedChannelId?: string
  thread?: { id: string; name: string } | null
}) {
  const location = useLocation()
  const navigate = useNavigate()
  const [creating, setCreating] = useState(false)
  const experience = useProjectExperience(project.id)
  const chat = experience.shows('area.chat')
  const threads = experience.shows('talk.threads')
  const updateNavigation = useUpdateProjectNavigation()
  const expandSelected = new URLSearchParams(location.search).get('threads') === 'all'
  const sidebar = useProjectConversationSidebar(project.id, selectedChannelId, {
    expandedChannelId: expandSelected && selectedChannelId ? selectedChannelId : undefined,
  })
  const base = `/projects/${encodeURIComponent(project.id)}`
  const canCreate = chat && Boolean(sidebar.data?.capabilities.createChannel)
  const go = (to: string) => {
    onNavigate()
    navigate(to)
  }
  const pinned = project.navigation.pinned
  // Keyboard users move a pin up and down the rail from here; others drag it.
  const projects = useProjects()
  const setRailOrder = useSetRailOrder()
  const rail = railProjects((projects.data?.projects ?? []).filter((candidate) => !candidate.archivedAt), project.id)
  const railSteps = pinned
    ? ([
        ['Move up in the rail', 'arrow-up', railOrderAfterStep(rail, project.id, -1)],
        ['Move down in the rail', 'arrow-down', railOrderAfterStep(rail, project.id, 1)],
      ] as const)
    : []
  const togglePin = () => {
    updateNavigation
      .mutateAsync({ projectId: project.id, expectedVersion: project.navigation.version, navigation: project.navigation, pinned: !pinned })
      .then(() => showToast({ tone: 'success', title: pinned ? `${project.name} unpinned from the rail` : `${project.name} pinned to the rail` }))
      .catch((reason: unknown) =>
        showToast({ tone: 'danger', title: 'The pin didn’t change', description: reason instanceof Error ? reason.message : undefined }),
      )
  }
  const menu: MenuEntry[] = [
    ...(canCreate ? [{ label: 'New channel', icon: 'plus' as const, onSelect: () => setCreating(true) }] : []),
    ...(project.capabilities.manage
      ? [{ label: 'Invite people', icon: 'user-plus' as const, onSelect: () => go(projectSettingsHref(project.id, 'members')) }]
      : []),
    ...(project.capabilities.navigationWrite
      ? [{ label: pinned ? 'Unpin from the rail' : 'Pin to the rail', icon: 'star' as const, onSelect: togglePin }]
      : []),
    ...(project.capabilities.navigationWrite
      ? railSteps.flatMap(([label, icon, order]) =>
          order ? [{ label, icon, onSelect: () => setRailOrder.mutate(order, { onError: (reason) => toastError('The rail’s order wasn’t saved', reason) }) }] : [],
        )
      : []),
    ...(chat ? [{ label: 'Archived conversations', icon: 'archive' as const, onSelect: () => go(`${base}/conversations/archived`) }] : []),
    { label: 'Project settings', icon: 'settings', onSelect: () => go(projectSettingsHref(project.id)) },
  ]
  const section = projectSection(location.pathname, base)
  const listedThread = sidebar.data?.channels.some((channel) => channel.threads.some((candidate) => candidate.id === thread?.id))
  // Each row is a part of the layout, and the layout's lead parts go first. Pages also holds Decisions.
  const pages = experience.shows('area.pages')
  const topLinks: { part: PartId; shown: boolean; link: ReactNode }[] = [
    {
      part: 'area.tasks',
      shown: experience.shows('area.tasks'),
      link: (
        <SidebarLink
          active={section === 'work'}
          icon="work"
          key="tasks"
          label={experience.label('task', 'many')}
          onNavigate={onNavigate}
          to={`${base}/work-items`}
        />
      ),
    },
    {
      part: 'area.pages',
      shown: pages || experience.shows('area.decisions'),
      link: (
        <SidebarLink
          active={section === 'pages'}
          icon="list"
          key="pages"
          label={pages ? experience.label('page', 'many') : 'Decisions'}
          onNavigate={onNavigate}
          to={`${base}/${pages ? 'pages' : 'decisions'}`}
        />
      ),
    },
    {
      part: 'area.collections',
      shown: experience.shows('area.collections'),
      link: <SidebarLink icon="table" key="collections" label="Collections" onNavigate={onNavigate} to={`${base}/collections`} />,
    },
    {
      part: 'area.sheets',
      shown: experience.shows('area.sheets'),
      link: <SidebarLink icon="table" key="sheets" label="Sheets" onNavigate={onNavigate} to={`${base}/sheets`} />,
    },
    {
      part: 'area.runs',
      shown: experience.shows('area.runs'),
      link: <SidebarLink icon="activity" key="runs" label="Runs" onNavigate={onNavigate} to={`${base}/launches`} />,
    },
  ]
  const leading = (part: PartId) => (experience.lead.includes(part) ? 0 : 1)
  const topRows = topLinks
    .filter((row) => row.shown)
    .sort((left, right) => leading(left.part) - leading(right.part))
    .map((row) => row.link)

  return (
    <SidebarFrame
      actions={
        <IconButton
          icon="search"
          label={`Search ${project.name}`}
          onClick={() => go(`/search?projectId=${encodeURIComponent(project.id)}`)}
          side="bottom"
        />
      }
      id="project-sidebar"
      label={`${project.name} navigation`}
      menu={menu}
      mobileOpen={mobileOpen}
      space={`project:${project.id}`}
      title={project.name}
    >
      <ul className="aw-section__list aw-sidebar__top">
        <SidebarLink end icon="home" label="Overview" onNavigate={onNavigate} to={base} />
        {topRows}
      </ul>
      {experience.shows('area.tasks') ? (
        <Suspense fallback={null}>
          <PinnedViews onNavigate={onNavigate} projectId={project.id} />
        </Suspense>
      ) : null}
      {experience.shows('area.agents') ? (
        <Suspense fallback={null}>
          <SidebarAgents onNavigate={onNavigate} projectId={project.id} />
        </Suspense>
      ) : null}
      {chat ? (
        <SidebarSection
          addLabel={`New ${experience.word('channel')}`}
          label={experience.label('channel', 'many')}
          onAdd={canCreate ? () => setCreating(true) : undefined}
        >
          {(sidebar.data?.channels ?? []).flatMap((channel) => {
            const selected = channel.id === selectedChannelId
            const rows = [
              <SidebarLink
                icon="hash"
                keep={selected || channel.unread}
                key={channel.id}
                label={channel.name}
                onNavigate={onNavigate}
                to={`${base}/channels/${encodeURIComponent(channel.id)}`}
                unread={channel.unread}
              />,
              ...(threads ? channel.threads : []).map((channelThread) => (
                <SidebarLink
                  attention={attentionMarks[channelThread.attention]}
                  attentionLabel={channelThread.attentionLabel}
                  dataAttention={channelThread.attention}
                  keep={channelThread.attention === 'response_required' || channelThread.id === thread?.id}
                  key={channelThread.id}
                  label={channelThread.name}
                  nested
                  onNavigate={onNavigate}
                  to={`${base}/threads/${encodeURIComponent(channelThread.id)}`}
                  unread={channelThread.attention === 'unread'}
                />
              )),
            ]
            if (threads && channel.hasMoreThreads) {
              rows.push(
                <SidebarLink
                  key={`${channel.id}-more`}
                  label="All threads"
                  nested
                  onNavigate={onNavigate}
                  to={`${base}/channels/${encodeURIComponent(channel.id)}?threads=all`}
                />,
              )
            }
            return rows
          })}
          {thread && sidebar.data && !listedThread ? (
            <SidebarLink
              keep
              key={thread.id}
              label={thread.name}
              nested
              onNavigate={onNavigate}
              to={`${base}/threads/${encodeURIComponent(thread.id)}`}
            />
          ) : null}
        </SidebarSection>
      ) : null}
      {chat && sidebar.isError ? <p className="aw-sidebar__note">Channels couldn’t load. They’ll retry on their own.</p> : null}
      {creating ? (
        <Suspense fallback={null}>
          <ChannelCreator onClose={() => setCreating(false)} onCreated={(to) => go(to)} open projectId={project.id} />
        </Suspense>
      ) : null}
    </SidebarFrame>
  )
}

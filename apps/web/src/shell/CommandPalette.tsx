import { useEffect, useMemo, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { clearAuthenticatedQueryCache, logout, useAuthenticatedSession } from '../auth/session'
import { useProjectConversationSidebar } from '../conversations/hooks'
import { conversationTitle, openDm, useDirectOverview } from '../direct/api'
import { projectLocationHref } from '../projects/locations'
import { projectSettingsHref } from '../settings/paths'
import { useProjectSearch, useProjects, useSwitchProject, useUpdateProjectNavigation } from '../projects/hooks'
import { ProjectInitials } from '../projects/ProjectInitials'
import type { Project } from '../projects/types'
import { THEME_PRESETS } from '../theme/slots'
import { useTheme } from '../theme/themeContext'
import { Avatar, Button, Dialog, showToast, type CommandItem } from '../ui'
import { CommandMenu } from '../ui/commandMenu'
import { searchWorkItemReferences } from '../work-items/api'
import { closeCommandMenu, openShortcutSheet, pageCommands, useCommandState } from './commandStore'
import { SHORTCUTS, parseTaskKey } from './shortcuts'
import { useProjectByKey } from './useProjectByKey'
import { projectPath, useRouteProject } from './useRouteProject'


function useSettled(value: string, delay = 150) {
  const [settled, setSettled] = useState(value)
  useEffect(() => {
    const timer = window.setTimeout(() => setSettled(value), delay)
    return () => window.clearTimeout(timer)
  }, [delay, value])
  return settled
}

/** Task search for the menu: by key across Projects, by words in the current one. */
function useTaskResults(query: string, current: Project | null) {
  const settled = useSettled(query.trim())
  const key = parseTaskKey(settled)
  const { project: keyProject } = useProjectByKey(key?.projectKey ?? null, current?.id)
  // A typed key searches only its own Project; words search the current one.
  const target = key ? keyProject : current
  const enabled = Boolean(target && settled.length >= 2)
  const results = useQuery({
    queryKey: ['project', target?.id ?? '', 'command-menu', 'tasks', settled],
    queryFn: async () => {
      if (!target) return []
      const results = await searchWorkItemReferences(target.id, settled)
      return results.map((result) => ({ ...result, projectName: target.name }))
    },
    enabled,
    staleTime: 15_000,
    retry: false,
  })
  // While the text is still settling, the results answer older text: hide them.
  return { data: settled === query.trim() ? results.data : undefined, isFetching: results.isFetching }
}

/**
 * The command menu and the shortcut sheet. Loaded on first open (and
 * prefetched when the browser is idle) by CommandCenter, which owns the
 * shortcuts that open them.
 */
export function CommandPalette() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const theme = useTheme()
  const state = useCommandState()
  const projectsQuery = useProjects()
  const projects = useMemo(
    () => (projectsQuery.data?.projects ?? []).filter((project) => !project.archivedAt),
    [projectsQuery.data?.projects],
  )
  const current = useRouteProject(projects)
  const direct = useDirectOverview()
  const { mutateAsync: updateNavigation } = useUpdateProjectNavigation()
  const { mutateAsync: switchProject } = useSwitchProject()
  const sidebar = useProjectConversationSidebar(state.open === 'commands' ? current?.id ?? '' : '')
  const [query, setQuery] = useState('')
  const tasks = useTaskResults(state.open === 'commands' ? query : '', current)
  // The rail lists the first 100 Projects; the server finds the rest by name.
  const projectSearch = useProjectSearch(state.open === 'commands' ? query : '')
  // Open with the requested text each time; forget it on close.
  const [openedWith, setOpenedWith] = useState<string | null>(null)
  if (state.open === 'commands' && openedWith === null) {
    setOpenedWith(state.initialQuery)
    setQuery(state.initialQuery)
  } else if (state.open !== 'commands' && openedWith !== null) {
    setOpenedWith(null)
    setQuery('')
  }


  const items = useMemo<CommandItem[]>(() => {
    if (state.open !== 'commands') return []
    const go = (to: string) => () => navigate(to)
    const list: CommandItem[] = []

    if (current) {
      const id = current.id
      list.push(
        { id: 'p:tasks', group: current.name, label: 'Tasks', icon: 'checklist', shortcut: 'G T', onSelect: go(projectPath(id, '/work-items')) },
        { id: 'p:board', group: current.name, label: 'Board', icon: 'board', shortcut: 'G B', onSelect: go(projectPath(id, '/work-items?view=board')) },
        { id: 'p:timeline', group: current.name, label: 'Timeline', icon: 'clock', keywords: ['gantt', 'roadmap', 'schedule', 'milestones'], onSelect: go(projectPath(id, '/timeline')) },
        { id: 'p:ready', group: current.name, label: 'Ready for agents', icon: 'bot', keywords: ['view', 'unclaimed'], onSelect: go(projectPath(id, '/work-items?ready=true&claimed=false&sort=priority')) },
        { id: 'p:overview', group: current.name, label: 'Overview', icon: 'home', onSelect: go(projectPath(id)) },
        { id: 'p:next', group: current.name, label: 'Up next', icon: 'play', keywords: ['ready', 'what to do', 'ranking', 'priorities'], onSelect: go(projectPath(id, '/next')) },
        { id: 'p:suggestions', group: current.name, label: 'AI suggestions', icon: 'bot', keywords: ['dependencies', 'blockers', 'review', 'triage'], onSelect: go(projectPath(id, '/suggestions')) },
        { id: 'p:graph', group: current.name, label: 'Dependency graph', icon: 'branch', keywords: ['blocked', 'critical path', 'dependencies'], onSelect: go(projectPath(id, '/graph')) },
        { id: 'p:plan-import', group: current.name, label: 'Import a plan', icon: 'list', keywords: ['roadmap', 'spec', 'plan to tasks'], onSelect: go(projectPath(id, '/plan-import')) },
        { id: 'p:pages', group: current.name, label: 'Pages', icon: 'list', shortcut: 'G P', onSelect: go(projectPath(id, '/pages')) },
        { id: 'p:decisions', group: current.name, label: 'Decisions', icon: 'circle-check', shortcut: 'G D', onSelect: go(projectPath(id, '/decisions')) },
        { id: 'p:archived', group: current.name, label: 'Archived conversations', icon: 'archive', onSelect: go(projectPath(id, '/conversations/archived')) },
        { id: 'p:settings', group: current.name, label: 'Project settings', icon: 'settings', shortcut: 'G S', keywords: ['members', 'repositories', 'github', 'agents', 'automation', 'environments'], onSelect: go(projectSettingsHref(id)) },
      )
      for (const channel of sidebar.data?.channels ?? []) {
        if (channel.archivedAt) continue
        list.push({
          id: `c:${channel.id}`,
          group: 'Channels',
          label: `#${channel.name}`,
          icon: 'hash',
          hint: channel.unread ? 'New messages' : undefined,
          onSelect: go(projectPath(id, `/channels/${encodeURIComponent(channel.id)}`)),
        })
        for (const thread of channel.threads) {
          if (thread.archivedAt) continue
          list.push({
            id: `t:${thread.id}`,
            group: 'Threads',
            label: thread.name,
            icon: 'messages',
            hint: `in #${channel.name}`,
            onSelect: go(projectPath(id, `/threads/${encodeURIComponent(thread.id)}`)),
          })
        }
      }
    }

    for (const task of tasks.data ?? []) {
      list.push({
        id: `w:${task.id}`,
        group: 'Tasks',
        label: task.title,
        hint: `${task.key} · ${task.projectName}`,
        icon: 'checklist',
        matched: true,
        onSelect: go(task.href),
      })
    }
    const typedKey = parseTaskKey(query)
    // The task page resolves Projects beyond the rail's first 100 and says when a key isn't found.
    if (typedKey && !(tasks.data ?? []).some((task) => task.key === typedKey.key)) {
      list.push({ id: `k:${typedKey.key}`, group: 'Tasks', label: `Open ${typedKey.key}`, icon: 'checklist', matched: true, onSelect: go(`/${typedKey.key}`) })
    }

    list.push(...pageCommands(state.page))

    if (current) {
      list.push({
        id: 'a:create-task',
        group: 'Actions',
        label: 'Create task',
        icon: 'plus',
        shortcut: 'C',
        keywords: ['new task', 'work item', 'issue'],
        onSelect: go(projectPath(current.id, '/work-items?create=task')),
      })
    }
    for (const project of projects) {
      if (project.id === current?.id) continue
      list.push({
        id: `a:create-task:${project.id}`,
        group: 'Actions',
        label: `Create task in ${project.name}`,
        icon: 'plus',
        keywords: ['new task'],
        onSelect: go(projectPath(project.id, '/work-items?create=task')),
      })
    }
    if (current?.capabilities.navigationWrite) {
      const project = current
      const pinned = project.navigation.pinned
      list.push({
        id: 'a:pin',
        group: 'Actions',
        label: pinned ? `Unpin ${project.name}` : `Pin ${project.name}`,
        icon: 'star',
        keywords: ['pin', 'favorite', 'rail'],
        onSelect: () => {
          updateNavigation({ projectId: project.id, expectedVersion: project.navigation.version, navigation: project.navigation, pinned: !pinned })
            .then(() => showToast({ tone: 'success', title: pinned ? `Unpinned ${project.name}` : `Pinned ${project.name}` }))
            .catch((reason: unknown) =>
              showToast({ tone: 'danger', title: 'Couldn’t update the pin.', description: reason instanceof Error ? reason.message : 'Try again.' }),
            )
        },
      })
    }
    if (session.user.capabilities.createProject) {
      list.push({ id: 'a:create-project', group: 'Actions', label: 'Create project', icon: 'plus', keywords: ['new project'], onSelect: go('/projects/new') })
    }
    if (direct.data?.capabilities.startConversation) {
      list.push({ id: 'a:new-message', group: 'Actions', label: 'New message', icon: 'compose', keywords: ['dm', 'group'], onSelect: go('/messages') })
    }
    list.push({ id: 'a:shortcuts', group: 'Actions', label: 'Keyboard shortcuts', icon: 'info', shortcut: '?', onSelect: openShortcutSheet })
    list.push({
      id: 'a:sign-out',
      group: 'Actions',
      label: 'Sign out',
      icon: 'log-out',
      onSelect: () => {
        void logout()
          .then(() => {
            clearAuthenticatedQueryCache(queryClient)
            navigate('/sign-in', { replace: true })
          })
          .catch((reason: unknown) =>
            showToast({ tone: 'danger', title: 'Couldn’t sign out.', description: reason instanceof Error ? reason.message : 'Try again.' }),
          )
      },
    })

    list.push(
      { id: 'g:home', group: 'Go to', label: 'Home', icon: 'home', shortcut: 'G H', onSelect: go('/home') },
      { id: 'g:my-work', group: 'Go to', label: 'My work', icon: 'checklist', shortcut: 'G W', keywords: ['assigned', 'tasks', 'review', 'claimed'], onSelect: go('/my-work') },
      { id: 'g:runs', group: 'Go to', label: 'Runs', icon: 'activity', keywords: ['agent runs', 'tokens', 'cost', 'usage', 'diff'], onSelect: go('/runs') },
      { id: 'g:whiteboards', group: 'Go to', label: 'Whiteboards', icon: 'whiteboard', keywords: ['excalidraw', 'draw', 'diagram', 'sketch', 'canvas'], onSelect: go('/whiteboards') },
      { id: 'g:messages', group: 'Go to', label: 'Messages', icon: 'message', shortcut: 'G M', keywords: ['inbox', 'dms'], onSelect: go('/messages') },
      { id: 'g:agents', group: 'Go to', label: 'Agents', icon: 'bot', onSelect: go('/messages/agents') },
      { id: 'g:search', group: 'Go to', label: 'Search', icon: 'search', shortcut: '/', onSelect: go('/search') },
      { id: 'g:appearance', group: 'Go to', label: 'Appearance', icon: 'palette', shortcut: 'G A', keywords: ['theme', 'dark', 'light', 'density'], onSelect: go('/settings/account/appearance') },
      { id: 'g:settings', group: 'Go to', label: 'Settings', icon: 'settings', keywords: ['account', 'profile', 'workspace', 'security', 'tokens'], onSelect: go('/settings') },
    )

    const listed = new Set(projects.map((project) => project.id))
    const found = (projectSearch.data?.projects ?? []).filter(
      (project) => !project.archivedAt && !listed.has(project.id),
    )
    for (const project of [...projects, ...found]) {
      list.push({
        id: `pr:${project.id}`,
        group: 'Projects',
        label: project.name,
        hint: project.key,
        mark: <ProjectInitials project={project} size={20} />,
        keywords: [project.key],
        matched: !listed.has(project.id),
        onSelect: () => {
          const href = projectLocationHref(project.id, project.navigation.location)
          if (!project.capabilities.navigationWrite) return navigate(href)
          // Record the switch so the rail's recent Projects stay in order.
          void switchProject({
            projectId: project.id,
            expectedVersion: project.navigation.version,
            navigation: project.navigation,
          }).then(
            (result) => navigate(result.href),
            () => navigate(href),
          )
        },
      })
    }

    for (const conversation of direct.data?.conversations ?? []) {
      const title = conversationTitle(conversation, session.user.actorId)
      list.push({
        id: `d:${conversation.id}`,
        group: 'Messages',
        label: title,
        icon: conversation.kind === 'group' ? 'users' : 'message',
        hint: conversation.unread ? 'Unread' : undefined,
        onSelect: go(`/messages/${encodeURIComponent(conversation.id)}`),
      })
    }
    if (direct.data?.capabilities.startConversation) {
      for (const person of direct.data.people) {
        if (person.id === session.user.actorId) continue
        list.push({
          id: `u:${person.id}`,
          group: 'People',
          label: `Message ${person.displayName}`,
          hint: `@${person.handle}`,
          mark: <Avatar name={person.displayName} seed={person.id} size={20} />,
          keywords: [person.displayName, person.handle],
          onSelect: () => {
            void openDm(person.id)
              .then((conversationId) => navigate(`/messages/${encodeURIComponent(conversationId)}`))
              .catch((reason: unknown) =>
                showToast({ tone: 'danger', title: 'Couldn’t open that conversation.', description: reason instanceof Error ? reason.message : 'Try again.' }),
              )
          },
        })
      }
    }

    list.push({
      id: 'theme:system',
      group: 'Theme',
      label: 'Use the system theme',
      icon: 'palette',
      keywords: ['theme', 'dark mode', 'light mode'],
      hint: theme.theme === 'system' ? 'Current' : undefined,
      onSelect: () => theme.setTheme('system'),
    })
    for (const preset of THEME_PRESETS) {
      list.push({
        id: `theme:${preset.id}`,
        group: 'Theme',
        label: `Switch to ${preset.name}`,
        icon: 'palette',
        keywords: ['theme', preset.id === 'daylight' ? 'light mode' : 'dark mode'],
        hint: theme.theme === preset.id ? 'Current' : undefined,
        onSelect: () => theme.setTheme(preset.id),
      })
    }
    return list
  }, [current, direct.data, navigate, projectSearch.data, projects, query, queryClient, session.user, sidebar.data, state.open, state.page, switchProject, tasks.data, theme, updateNavigation])

  // With nothing typed, show where you'd most likely go; everything else is a search away.
  const visibleItems = useMemo(() => {
    if (query.trim()) return items
    const quiet = new Set(['Threads', 'People', 'Theme'])
    return items.filter((item) => !quiet.has(item.group) && !item.id.startsWith('a:create-task:'))
  }, [items, query])

  return (
    <>
      {state.open === 'commands' ? (
        <CommandMenu
          footer={
            <>
              <span>↑↓ to move</span>
              <span>↵ to open</span>
              <span>Type a task key like {current?.key ?? 'AWD'}-12</span>
            </>
          }
          items={visibleItems}
          label="Command menu"
          loading={tasks.isFetching}
          onClose={closeCommandMenu}
          onQueryChange={setQuery}
          placeholder={current ? `Search ${current.name}, go anywhere, or run a command` : 'Go anywhere or run a command'}
          query={query}
        />
      ) : null}
      <ShortcutSheet inProject={Boolean(current)} onClose={closeCommandMenu} open={state.open === 'shortcuts'} />
    </>
  )
}

function ShortcutSheet({ inProject, onClose, open }: { inProject: boolean; onClose: () => void; open: boolean }) {
  const groups = ['General', 'Go to'] as const
  return (
    <Dialog
      description={inProject ? undefined : 'Open a Project for its Tasks, Board, Pages and settings shortcuts.'}
      footer={<Button onClick={onClose}>Close</Button>}
      onClose={onClose}
      open={open}
      title="Keyboard shortcuts"
      width={640}
    >
      <div className="aw-shortcuts">
        {groups.map((group) => (
          <section key={group}>
            <h3>{group}</h3>
            <dl>
              {SHORTCUTS.filter((shortcut) => shortcut.group === group).map((shortcut) => (
                <div className="aw-shortcuts__row" key={shortcut.action} style={{ display: 'contents' }}>
                  <dt>{shortcut.label}</dt>
                  <dd aria-label={shortcut.keys.length === 2 && shortcut.keys[0] === 'G' ? `G then ${shortcut.keys[1]}` : shortcut.keys.join(' ')}>
                    {shortcut.keys.map((key, index) => (
                      <kbd key={index}>{key}</kbd>
                    ))}
                  </dd>
                </div>
              ))}
            </dl>
          </section>
        ))}
      </div>
    </Dialog>
  )
}

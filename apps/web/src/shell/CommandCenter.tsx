import { useEffect, useMemo, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useLocation, useNavigate } from 'react-router-dom'

import { clearAuthenticatedQueryCache, logout, useAuthenticatedSession } from '../auth/session'
import { useProjectConversationSidebar } from '../conversations/hooks'
import { conversationTitle, openDm, useDirectOverview } from '../direct/api'
import { projectLocationHref } from '../projects/locations'
import { useProjectSearch, useProjects, useUpdateProjectNavigation } from '../projects/hooks'
import { ProjectInitials } from '../projects/ProjectInitials'
import type { Project } from '../projects/types'
import { THEME_PRESETS } from '../theme/slots'
import { useTheme } from '../theme/themeContext'
import { Avatar, Button, CommandMenu, Dialog, showToast, type CommandItem } from '../ui'
import { searchWorkItemReferences } from '../work-items/api'
import { closeCommandMenu, openCommandMenu, openShortcutSheet, pageCommands, useCommandState } from './commandStore'
import { SHORTCUTS, createShortcutMatcher, parseTaskKey, type ShortcutAction } from './shortcuts'

const projectPath = (projectId: string, rest = '') => `/projects/${encodeURIComponent(projectId)}${rest}`

/** The Project in the address bar, if any. */
function useRouteProject(projects: Project[]) {
  const location = useLocation()
  const match = /^\/projects\/([^/]+)/.exec(location.pathname)
  const id = match ? decodeURIComponent(match[1]) : ''
  return projects.find((project) => project.id === id) ?? null
}

function useSettled(value: string, delay = 150) {
  const [settled, setSettled] = useState(value)
  useEffect(() => {
    const timer = window.setTimeout(() => setSettled(value), delay)
    return () => window.clearTimeout(timer)
  }, [delay, value])
  return settled
}

/** Task search for the menu: by key across Projects, by words in the current one. */
function useTaskResults(query: string, projects: Project[], current: Project | null) {
  const settled = useSettled(query.trim())
  const key = parseTaskKey(settled)
  const keyProject = key ? projects.find((project) => project.key.toUpperCase() === key.projectKey) ?? null : null
  const target = keyProject ?? current
  const enabled = Boolean(target && settled.length >= 2)
  return useQuery({
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
}

/**
 * Cmd+K from anywhere: every page, Project, channel, conversation, person and
 * common action, plus what the current page offers. Also owns the global
 * keyboard shortcuts and the shortcut sheet.
 */
export function CommandCenter() {
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
  const sidebar = useProjectConversationSidebar(state.open === 'commands' ? current?.id ?? '' : '')
  const [query, setQuery] = useState('')
  const tasks = useTaskResults(state.open === 'commands' ? query : '', projects, current)
  // The rail lists the first 100 Projects; the server finds the rest by name.
  const projectSearch = useProjectSearch(state.open === 'commands' ? query : '')
  const currentRef = useRef(current)
  useEffect(() => {
    currentRef.current = current
  }, [current])

  // Open with the requested text each time; forget it on close.
  const [openedWith, setOpenedWith] = useState<string | null>(null)
  if (state.open === 'commands' && openedWith === null) {
    setOpenedWith(state.initialQuery)
    setQuery(state.initialQuery)
  } else if (state.open !== 'commands' && openedWith !== null) {
    setOpenedWith(null)
    setQuery('')
  }

  useEffect(() => {
    const press = createShortcutMatcher()
    const go = (to: string) => navigate(to)
    const inProject = (rest: string) => {
      const project = currentRef.current
      if (project) go(projectPath(project.id, rest))
      else openCommandMenu()
    }
    const handlers: Record<ShortcutAction, () => void> = {
      'command-menu': () => openCommandMenu(),
      'shortcut-sheet': openShortcutSheet,
      'create-task': () => {
        const project = currentRef.current
        if (project) go(projectPath(project.id, '/work-items?create=task'))
        else openCommandMenu('Create task in')
      },
      search: () => go('/search'),
      'go-home': () => go('/home'),
      // Messages holds what needs you until the inbox arrives (W6).
      'go-inbox': () => go('/messages'),
      'go-messages': () => go('/messages'),
      'go-tasks': () => inProject('/work-items'),
      'go-board': () => inProject('/work-items?view=board'),
      'go-pages': () => inProject('/pages'),
      'go-decisions': () => inProject('/decisions'),
      'go-settings': () => inProject('/settings'),
      'go-appearance': () => go('/settings/appearance'),
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return
      const action = press(event)
      if (!action) return
      event.preventDefault()
      if (action === 'command-menu' && document.querySelector('.aw-cmdk')) {
        closeCommandMenu()
        return
      }
      handlers[action]()
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [navigate])

  const items = useMemo<CommandItem[]>(() => {
    if (state.open !== 'commands') return []
    const go = (to: string) => () => navigate(to)
    const list: CommandItem[] = []

    if (current) {
      const id = current.id
      list.push(
        { id: 'p:tasks', group: current.name, label: 'Tasks', icon: 'checklist', shortcut: 'G T', onSelect: go(projectPath(id, '/work-items')) },
        { id: 'p:board', group: current.name, label: 'Board', icon: 'board', shortcut: 'G B', onSelect: go(projectPath(id, '/work-items?view=board')) },
        { id: 'p:overview', group: current.name, label: 'Overview', icon: 'home', onSelect: go(projectPath(id)) },
        { id: 'p:pages', group: current.name, label: 'Pages', icon: 'list', shortcut: 'G P', onSelect: go(projectPath(id, '/pages')) },
        { id: 'p:decisions', group: current.name, label: 'Decisions', icon: 'circle-check', shortcut: 'G D', onSelect: go(projectPath(id, '/decisions')) },
        { id: 'p:archived', group: current.name, label: 'Archived conversations', icon: 'archive', onSelect: go(projectPath(id, '/conversations/archived')) },
        { id: 'p:settings', group: current.name, label: 'Project settings', icon: 'settings', shortcut: 'G S', keywords: ['members', 'repositories', 'github', 'agents'], onSelect: go(projectPath(id, '/settings')) },
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
    if (typedKey && projects.some((project) => project.key.toUpperCase() === typedKey.projectKey) && !(tasks.data ?? []).some((task) => task.key === typedKey.key)) {
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
      { id: 'g:messages', group: 'Go to', label: 'Messages', icon: 'message', shortcut: 'G M', keywords: ['inbox', 'dms'], onSelect: go('/messages') },
      { id: 'g:agents', group: 'Go to', label: 'Agents', icon: 'bot', onSelect: go('/messages/agents') },
      { id: 'g:search', group: 'Go to', label: 'Search', icon: 'search', shortcut: '/', onSelect: go('/search') },
      { id: 'g:appearance', group: 'Go to', label: 'Appearance', icon: 'palette', shortcut: 'G A', keywords: ['theme', 'dark', 'light', 'density'], onSelect: go('/settings/appearance') },
      { id: 'g:account', group: 'Go to', label: 'Account settings', icon: 'settings', keywords: ['profile', 'password', 'connected apps'], onSelect: go('/settings/account') },
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
        onSelect: go(projectLocationHref(project.id, project.navigation.location)),
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
  }, [current, direct.data, navigate, projectSearch.data, projects, query, queryClient, session.user, sidebar.data, state.open, state.page, tasks.data, theme, updateNavigation])

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

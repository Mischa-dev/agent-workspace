import { useEffect, useMemo, useState } from 'react'
import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'
import { useLocation, useNavigate } from 'react-router-dom'
import paletteClaims from 'virtual:modules/slots/palette'
import slashClaims from 'virtual:modules/slots/slash'

import { clearAuthenticatedQueryCache, logout, useAuthenticatedSession } from '../auth/session'
import { useProjectConversationSidebar } from '../conversations/hooks'
import { useHomeExperience, useHomeWorkspaceId, useProjectExperience, type Experience } from '../experience/layout'
import { conversationTitle, openDm, useDirectOverview } from '../direct/api'
import { projectLocationHref } from '../projects/locations'
import { projectSettingsHref } from '../settings/paths'
import { useProjectSearch, useProjects, useSwitchProject, useUpdateProjectNavigation } from '../projects/hooks'
import { moduleSwitchable, ownerOf } from '../modules/ownership'
import { moduleOfPath } from '../modules/pageOwners'
import { ProjectInitials } from '../projects/ProjectInitials'
import type { Project } from '../projects/types'
import { THEME_PRESETS } from '../theme/slots'
import { useTheme } from '../theme/themeContext'
import { Avatar, Button, Dialog, countLabel, isMac, showToast, type CommandItem } from '../ui'
import { CommandMenu } from '../ui/commandMenu'
import { SLASH_COMMANDS } from '../later/slashCommands'
import { searchWorkItemReferences } from '../work-items/api'
import { createTaskHref } from '../work-items/links'
import { BOARD_SHORTCUTS, LIST_SHORTCUTS } from '../work-items/shortcutKeys'
import { closeCommandMenu, openMarkAllRead, openShortcutSheet, pageCommands, useCommandState } from './commandStore'
import { markAllRead } from './markAllRead'
import { pauseChoices, pausedUntilLabel, useNotificationPause } from './notificationPause'
import { placeIcon, readRecentPlaces } from './recentPlaces'
import { shortcutOn } from './shortcutModules'
import { SHORTCUTS } from './shortcutSheet'
import { parseTaskKey } from './shortcuts'
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
      return results.map((result) => ({ ...result, projectName: target.name, projectId: target.id }))
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
  const location = useLocation()
  const queryClient = useQueryClient()
  const theme = useTheme()
  const notificationPause = useNotificationPause()
  const state = useCommandState()
  const projectsQuery = useProjects()
  const projects = useMemo(
    () => (projectsQuery.data?.projects ?? []).filter((project) => !project.archivedAt),
    [projectsQuery.data?.projects],
  )
  const current = useRouteProject(projects)
  // AWD-75: entries of modules that are off for the person don't show, nor
  // those of a module that can be off while the person's settings load.
  const projectExperience = useProjectExperience(current?.id)
  const homeExperience = useHomeExperience()
  const homeId = useHomeWorkspaceId()
  const direct = useDirectOverview()
  const { mutateAsync: updateNavigation } = useUpdateProjectNavigation()
  const { mutateAsync: switchProject } = useSwitchProject()
  const sidebar = useProjectConversationSidebar(
    state.open === 'commands' && !current?.modulesOff?.includes('channels') ? current?.id ?? '' : '',
  )
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

    // Where you were last, for opening the menu and pressing Enter.
    if (!query.trim()) {
      const here = location.pathname
      for (const place of readRecentPlaces(session.user.actorId).filter((place) => place.path !== here).slice(0, 6)) {
        list.push({ id: `r:${place.path}`, group: 'Recent', label: place.title, icon: placeIcon(place.path), onSelect: go(place.path), ...destination(place.path, queryClient, homeId) })
      }
    }

    if (current) {
      const id = current.id
      list.push(
        { id: 'p:overview', group: current.name, label: 'Overview', icon: 'home', onSelect: go(projectPath(id)) },
        { id: 'p:tasks', group: current.name, label: 'Tasks', icon: 'checklist', shortcut: 'G T', keywords: ['list'], onSelect: go(projectPath(id, '/work-items')) },
        { id: 'p:board', group: current.name, label: 'Board', icon: 'board', shortcut: 'G B', onSelect: go(projectPath(id, '/work-items?view=board')) },
        { id: 'p:pages', group: current.name, label: 'Pages', icon: 'list', shortcut: 'G P', keywords: ['docs', 'wiki'], onSelect: go(projectPath(id, '/pages')) },
        { id: 'p:collections', group: current.name, label: 'Collections', icon: 'table', keywords: ['records', 'tables', 'database', 'spreadsheet'], onSelect: go(projectPath(id, '/collections')) },
        { id: 'p:sheets', group: current.name, label: 'Sheets', icon: 'table', keywords: ['spreadsheet', 'formulas', 'cells', 'budget'], onSelect: go(projectPath(id, '/sheets')) },
        { id: 'p:milestones', group: current.name, label: 'Milestones', icon: 'checklist', keywords: ['releases', 'goals'], onSelect: go(projectPath(id, '/milestones')) },
        { id: 'p:timeline', group: current.name, label: 'Timeline', icon: 'clock', keywords: ['gantt', 'roadmap', 'schedule', 'milestones'], onSelect: go(projectPath(id, '/timeline')) },
        { id: 'p:next', group: current.name, label: 'Planning', icon: 'branch', keywords: ['up next', 'ready', 'what to do', 'ranking', 'priorities'], onSelect: go(projectPath(id, '/next')) },
        { id: 'p:ready', group: current.name, label: 'Ready for agents', icon: 'bot', keywords: ['view', 'unclaimed'], onSelect: go(projectPath(id, '/work-items?ready=true&claimed=false&sort=priority')) },
        { id: 'p:runs', group: current.name, label: 'Runs', icon: 'activity', keywords: ['launches', 'launch queue', 'agent runs'], onSelect: go(projectPath(id, '/launches')) },
        { id: 'p:suggestions', group: current.name, label: 'Suggestions', icon: 'bot', keywords: ['dependencies', 'blockers', 'review', 'triage'], onSelect: go(projectPath(id, '/suggestions')) },
        { id: 'p:graph', group: current.name, label: 'Dependency graph', icon: 'branch', keywords: ['blocked', 'critical path', 'dependencies'], onSelect: go(projectPath(id, '/graph')) },
        { id: 'p:plan-import', group: current.name, label: 'Import a plan', icon: 'list', keywords: ['roadmap', 'spec', 'plan to tasks'], onSelect: go(projectPath(id, '/plan-import')) },
        { id: 'p:decisions', group: current.name, label: 'Decisions', icon: 'circle-check', shortcut: 'G D', onSelect: go(projectPath(id, '/decisions')) },
        { id: 'p:archived', group: current.name, label: 'Archived conversations', icon: 'archive', onSelect: go(projectPath(id, '/conversations/archived')) },
        { id: 'p:settings', group: current.name, label: 'Project settings', icon: 'settings', shortcut: 'G S', keywords: ['members', 'invite', 'repositories', 'github', 'agents', 'automation', 'environments'], onSelect: go(projectSettingsHref(id)) },
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
        project: task.projectId,
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

    // The menu matches names; messages, pages and decisions are a full search away.
    const typed = query.trim()
    if (typed) {
      const words = new URLSearchParams({ q: typed })
      if (current) {
        list.push({
          id: 's:project',
          group: 'Search',
          label: `Search ${current.name} for “${typed}”`,
          icon: 'search',
          matched: true,
          onSelect: go(`/search?${words}&projectId=${encodeURIComponent(current.id)}`),
        })
      }
      list.push({ id: 's:everywhere', group: 'Search', label: `Search everything for “${typed}”`, icon: 'search', matched: true, onSelect: go(`/search?${words}`) })
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
        // Keeps the open tasks view (filters, board, open task) behind the dialog.
        onSelect: () => navigate(createTaskHref(current.id, window.location)),
      })
    }
    for (const project of projects) {
      if (project.id === current?.id) continue
      list.push({
        id: `a:create-task:${project.id}`,
        project: project.id,
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
        label: pinned ? `Unpin ${project.name} from the rail` : `Pin ${project.name} to the rail`,
        icon: 'star',
        keywords: ['pin', 'favorite', 'rail'],
        onSelect: () => {
          updateNavigation({ projectId: project.id, expectedVersion: project.navigation.version, navigation: project.navigation, pinned: !pinned })
            .then(() => showToast({ tone: 'success', title: pinned ? `${project.name} unpinned from the rail` : `${project.name} pinned to the rail` }))
            .catch((reason: unknown) =>
              showToast({ tone: 'danger', title: 'The pin didn’t change', description: reason instanceof Error ? reason.message : 'Try again.' }),
            )
        },
      })
    }
    if (session.user.capabilities.createProject) {
      list.push({ id: 'a:create-project', group: 'Actions', label: 'New project', icon: 'plus', keywords: ['create project'], onSelect: go('/projects/new') })
    }
    list.push({ id: 'a:setup', group: 'Actions', label: 'Workspace setup', icon: 'checklist', keywords: ['onboarding', 'welcome', 'getting started', 'set up'], onSelect: go('/welcome') })
    if (direct.data?.capabilities.startConversation) {
      list.push({ id: 'a:new-message', group: 'Actions', label: 'New message', icon: 'compose', keywords: ['dm', 'group'], onSelect: go('/messages') })
    }
    // The composer's slash commands that have a place outside a conversation.
    for (const command of SLASH_COMMANDS) {
      if (!command.paletteHref) continue
      list.push({
        id: `slash:${command.name}`,
        group: 'Actions',
        label: command.description,
        icon: command.icon,
        hint: `/${command.name}`,
        keywords: [`/${command.name}`, command.name],
        onSelect: go(command.paletteHref),
        // AWD-75: these lead to Later, so they follow Home's Inbox, not the open project.
        ...destination(command.paletteHref, queryClient, homeId),
      })
    }
    list.push({
      id: 'a:mark-all-read',
      group: 'Actions',
      label: 'Mark all messages read',
      icon: 'check',
      shortcut: '⇧ Esc',
      keywords: ['read all', 'clear unread', 'catch up'],
      onSelect: openMarkAllRead,
    })
    if (notificationPause.available) {
      const pauseFailed = (reason: unknown) =>
        showToast({ tone: 'danger', title: 'Notifications didn’t change', description: reason instanceof Error ? reason.message : 'Try again.' })
      const pauseWords = ['pause', 'do not disturb', 'dnd', 'snooze', 'quiet', 'mute', 'notifications']
      if (notificationPause.until) {
        list.push({
          id: 'a:resume-notifications',
          group: 'Actions',
          label: 'Resume notifications',
          hint: `Paused until ${pausedUntilLabel(notificationPause.until, new Date())}`,
          icon: 'play',
          keywords: pauseWords,
          onSelect: () => void notificationPause.resume().then(() => showToast({ tone: 'success', title: 'Notifications are back on' }), pauseFailed),
        })
      } else {
        for (const choice of pauseChoices(new Date())) {
          list.push({
            id: `a:pause-notifications:${choice.id}`,
            group: 'Actions',
            label: `Pause notifications ${choice.label.charAt(0).toLowerCase()}${choice.label.slice(1)}`,
            icon: 'moon',
            keywords: pauseWords,
            onSelect: () =>
              void notificationPause
                .pause(choice.until)
                .then(() => showToast({ tone: 'success', title: `Notifications paused until ${pausedUntilLabel(choice.until, new Date())}` }), pauseFailed),
          })
        }
      }
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
            showToast({ tone: 'danger', title: 'You’re still signed in', description: reason instanceof Error ? reason.message : 'Try again.' }),
          )
      },
    })

    list.push(
      { id: 'g:home', group: 'Go to', label: 'Home', icon: 'home', shortcut: 'G H', onSelect: go('/home') },
      { id: 'g:inbox', group: 'Go to', label: 'Inbox', icon: 'inbox', shortcut: 'G I', keywords: ['notifications', 'mentions', 'needs you'], onSelect: go('/inbox') },
      { id: 'g:my-work', group: 'Go to', label: 'My work', icon: 'checklist', shortcut: 'G W', keywords: ['assigned', 'tasks', 'review', 'claimed'], onSelect: go('/my-work') },
      { id: 'g:later', group: 'Go to', label: 'Later', icon: 'bookmark', shortcut: 'G L', keywords: ['saved', 'bookmarks', 'reminders', 'scheduled'], onSelect: go('/later') },
      { id: 'g:runs', group: 'Go to', label: 'Runs', icon: 'activity', keywords: ['agent runs', 'tokens', 'cost', 'usage', 'diff'], onSelect: go('/runs') },
      { id: 'g:whiteboards', group: 'Go to', label: 'Whiteboards', icon: 'whiteboard', keywords: ['excalidraw', 'draw', 'diagram', 'sketch', 'canvas'], onSelect: go('/whiteboards') },
      { id: 'g:messages', group: 'Go to', label: 'Messages', icon: 'message', shortcut: 'G M', keywords: ['dms', 'direct messages', 'groups'], onSelect: go('/messages') },
      { id: 'g:projects', group: 'Go to', label: 'All projects', icon: 'list', keywords: ['projects', 'archived'], onSelect: go('/projects') },
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
        project: project.id,
        group: 'Projects',
        label: project.name,
        hint: project.key,
        mark: <ProjectInitials project={project} size={20} />,
        keywords: [project.key],
        matched: !listed.has(project.id),
        onSelect: () => {
          navigate(projectLocationHref(project.id, project.navigation.location))
          // Recorded in the background, so the rail knows it was opened.
          if (project.capabilities.navigationWrite) {
            switchProject({ projectId: project.id, expectedVersion: project.navigation.version, navigation: project.navigation }).catch(() => undefined)
          }
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
                showToast({ tone: 'danger', title: `The conversation with ${person.displayName} didn’t open`, description: reason instanceof Error ? reason.message : 'Try again.' }),
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
  }, [current, direct.data, homeId, location.pathname, navigate, notificationPause, projectSearch.data, projects, query, queryClient, session.user, sidebar.data, state.open, state.page, switchProject, tasks.data, theme, updateNavigation])

  // AWD-75: an entry follows the module of where it leads, there: in another
  // project, that project's modules off (from the projects lists, so no
  // request per project); the open project's own entries, its settings; and
  // the rest, Home's. Not while those settings load. A typed task key opens
  // its task's project, whose own page decides.
  const shownItems = useMemo(() => {
    const listed = new Map([...(projectSearch.data?.projects ?? []), ...projects].map((project) => [project.id, project]))
    return items.filter((item) => {
      const module = item.module ?? paletteModule(item.id)
      if (!module || !moduleSwitchable(module) || item.ownerUnknown || item.id.startsWith('k:')) return true
      if (item.project && item.project !== current?.id) return !listed.get(item.project)?.modulesOff?.includes(module)
      const there = current && (item.project || IN_PROJECT.test(item.id)) ? projectExperience : homeExperience
      return there.modules !== 'loading' && there.moduleOn(module)
    })
  }, [current, homeExperience, items, projectExperience, projectSearch.data, projects])

  // With nothing typed, show where you'd most likely go; everything else is a search away.
  const visibleItems = useMemo(() => {
    if (query.trim()) return shownItems
    const quiet = new Set(['Threads', 'People', 'Theme'])
    // The project's main places; the rest (Timeline, Graph, Import a plan…) come up as you type.
    const projectFirst = new Set(['p:overview', 'p:tasks', 'p:board', 'p:pages'])
    return shownItems.filter(
      (item) =>
        !quiet.has(item.group) &&
        !(item.id.startsWith('p:') && !projectFirst.has(item.id)) &&
        item.id !== 'a:setup' &&
        !item.id.startsWith('a:create-task:') &&
        !item.id.startsWith('a:pause-notifications:'),
    )
  }, [shownItems, query])

  return (
    <>
      {state.open === 'commands' ? (
        <CommandMenu
          footer={
            <>
              <span>↑↓ to move</span>
              <span>↵ to open</span>
              <span>Type a task key, like {current?.key ?? 'AWD'}-12</span>
            </>
          }
          items={visibleItems}
          label="Command menu"
          loading={tasks.isFetching}
          onClose={closeCommandMenu}
          onQueryChange={setQuery}
          placeholder="Go anywhere, find a task, or run a command"
          query={query}
        />
      ) : null}
      <ShortcutSheet
        home={homeExperience}
        inProject={Boolean(current)}
        onClose={closeCommandMenu}
        open={state.open === 'shortcuts'}
        project={current ? projectExperience : null}
        taskView={taskViewAt(location.pathname, location.search)}
      />
      <MarkAllReadDialog onClose={closeCommandMenu} open={state.open === 'mark-all-read'} projectIds={projects.map((project) => project.id)} />
    </>
  )
}

/** The task list or board on screen, whose own keys the sheet lists too. */
function taskViewAt(pathname: string, search: string): 'list' | 'board' | null {
  if (!/^\/projects\/[^/]+\/work-items\/?$/.test(pathname)) return null
  return new URLSearchParams(search).get('view') === 'board' ? 'board' : 'list'
}

function ShortcutSheet({
  home,
  inProject,
  onClose,
  open,
  project,
  taskView,
}: {
  /** AWD-75: the settings the keys follow (shortcutOn), so the sheet lists what they do. */
  home: Experience
  inProject: boolean
  onClose: () => void
  open: boolean
  project: Experience | null
  taskView: 'list' | 'board' | null
}) {
  const groups = ['General', 'Go to', 'Conversations'] as const
  // The sheet already says how to open itself.
  const taskKeys = (taskView === 'board' ? BOARD_SHORTCUTS : LIST_SHORTCUTS).filter((shortcut) => shortcut.keys.join() !== '?')
  return (
    <Dialog
      description={
        !inProject
          ? 'Open a project for its Tasks, Board, Pages and settings shortcuts.'
          : !taskView
            ? 'The task list and board have keys of their own, shown here when one is open.'
            : undefined
      }
      footer={<Button onClick={onClose}>Close</Button>}
      onClose={onClose}
      open={open}
      title="Keyboard shortcuts"
      width={640}
    >
      <div className="aw-shortcuts">
        {taskView ? (
          <section>
            <h3>{taskView === 'board' ? 'Task board' : 'Task list'}</h3>
            <dl>
              {taskKeys.map((shortcut) => (
                <div className="aw-shortcuts__row" key={shortcut.label} style={{ display: 'contents' }}>
                  <dt>{shortcut.label}</dt>
                  <dd>
                    {shortcut.keys.map((key, index) => (
                      <kbd key={index}>{key}</kbd>
                    ))}
                  </dd>
                </div>
              ))}
            </dl>
          </section>
        ) : null}
        {groups.map((group) => (
          <section key={group}>
            <h3>{group}</h3>
            <dl>
              {SHORTCUTS.filter((shortcut) => shortcut.group === group && shortcutOn(shortcut.action, project, home)).map((shortcut) => (
                <div className="aw-shortcuts__row" key={shortcut.action} style={{ display: 'contents' }}>
                  <dt>{shortcut.label}</dt>
                  <dd aria-label={shortcut.keys.length === 2 && shortcut.keys[0] === 'G' ? `G then ${shortcut.keys[1]}` : shortcut.keys.map(spokenKey).join(' ')}>
                    {shortcut.keys.map((key, index) => (
                      <kbd key={index}>{shownKey(key)}</kbd>
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

const SPOKEN_KEYS: Record<string, string> = isMac
  ? { '⌘': 'Command', '⌥': 'Option', '⇧': 'Shift', '↑': 'Up', '↓': 'Down', Esc: 'Escape' }
  : { '⌘': 'Control', '⌥': 'Alt', '⇧': 'Shift', '↑': 'Up', '↓': 'Down', Esc: 'Escape' }

/** The sheet's keys are written for a Mac; elsewhere they read Ctrl, Alt and Shift. */
const OTHER_KEYS: Record<string, string> = { '⌘': 'Ctrl', '⌥': 'Alt', '⇧': 'Shift' }

function shownKey(key: string) {
  return isMac ? key : (OTHER_KEYS[key] ?? key)
}

function spokenKey(key: string) {
  return SPOKEN_KEYS[key] ?? key
}

/** Asks first: there's no undo for marking everything read. */
function MarkAllReadDialog({ onClose, open, projectIds }: { onClose: () => void; open: boolean; projectIds: string[] }) {
  const queryClient = useQueryClient()
  const [busy, setBusy] = useState(false)
  const run = async () => {
    setBusy(true)
    try {
      const { marked, failed } = await markAllRead(projectIds)
      await queryClient.invalidateQueries()
      if (failed) {
        showToast({ tone: 'danger', title: `${countLabel(marked, 'conversation')} marked read, but some weren’t`, description: 'Try again in a moment.' })
      } else {
        showToast({ tone: 'success', title: marked ? `${countLabel(marked, 'conversation')} marked read` : 'Everything was already read' })
      }
      onClose()
    } catch (reason) {
      showToast({ tone: 'danger', title: 'Conversations weren’t marked read', description: reason instanceof Error ? reason.message : 'Try again.' })
    } finally {
      setBusy(false)
    }
  }
  return (
    <Dialog
      description="Every channel, thread, DM and group you can see will be marked read. Your inbox doesn’t change."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} onClick={() => void run()} variant="primary">
            Mark messages read
          </Button>
        </>
      }
      onClose={onClose}
      open={open}
      title="Mark all messages read?"
    />
  )
}

/** AWD-75: entries that act in the open project; the rest lead across the workspace. */
const IN_PROJECT = /^(p:|c:|t:|s:project$|a:create-task$|a:pin$)/

/** Where a board or run is, as its page's cached answer says. */
type Owner = { projectId?: string | null; organizationId?: string; whiteboard?: Owner }

/**
 * AWD-75: where a place leads: its page's module, and its project. A run's
 * or a board's comes from its page's cached answer (no request here); a
 * board across Home's workspace follows Home; one not known here is left to
 * its own page.
 */
function destination(href: string, queryClient: QueryClient, homeId: string | null): Pick<CommandItem, 'module' | 'project' | 'ownerUnknown'> {
  const path = href.split(/[?#]/)[0]
  const module = moduleOfPath(path) ?? undefined
  const project = /^\/projects\/([^/]+)/.exec(path)?.[1]
  if (project) return { module, project }
  const [, kind, id] = /^\/(runs|whiteboards)\/([^/]+)/.exec(path) ?? []
  if (!kind) return { module }
  const cached = queryClient.getQueryData<Owner>([kind, 'detail', id])
  const owner = cached?.whiteboard ?? cached
  if (owner?.projectId) return { module, project: owner.projectId }
  return owner?.organizationId && owner.organizationId === homeId ? { module } : { module, ownerUnknown: true }
}

/** The module a palette entry belongs to; slash commands follow their own. */
function paletteModule(id: string) {
  return id.startsWith('slash:') ? ownerOf(slashClaims, id.slice('slash:'.length)) : ownerOf(paletteClaims, id)
}

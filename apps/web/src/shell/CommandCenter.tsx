import { lazy, Suspense, useEffect, useMemo, useRef } from 'react'
import { useNavigate } from 'react-router-dom'
import shortcutClaims from 'virtual:modules/slots/shortcuts'

import { useHomeExperience, useProjectExperience } from '../experience/layout'
import { moduleSwitchable, ownerOf } from '../modules/ownership'
import { useProjects } from '../projects/hooks'
import { projectSettingsHref } from '../settings/paths'
import { createTaskHref } from '../work-items/links'
import { closeCommandMenu, openCommandMenu, openMarkAllRead, openShortcutSheet, useCommandState } from './commandStore'
import { loadShellLazy } from './loadShellLazy'
import { createShortcutMatcher, type ShortcutAction } from './shortcuts'
import { projectPath, useRouteProject } from './useRouteProject'

const loadPalette = () => import('./CommandPalette')

/** ⌥↑ and ⌥↓, loaded on first use to keep them out of the first page load. */
function moveInSidebar(direction: 1 | -1, unreadOnly: boolean) {
  void loadShellLazy().then((module) => module.moveInSidebar(direction, unreadOnly))
}
const CommandPalette = lazy(() => loadPalette().then((module) => ({ default: module.CommandPalette })))

/**
 * Cmd+K from anywhere, and the global keyboard shortcuts. The menu and the
 * shortcut sheet (CommandPalette) load on first open and are prefetched once
 * the browser is idle, so they stay out of the first page load.
 */
export function CommandCenter() {
  const navigate = useNavigate()
  const state = useCommandState()
  const projectsQuery = useProjects()
  const projects = useMemo(
    () => (projectsQuery.data?.projects ?? []).filter((project) => !project.archivedAt),
    [projectsQuery.data?.projects],
  )
  const current = useRouteProject(projects)
  const currentRef = useRef(current)
  // AWD-75: shortcuts of modules that are off for the person do nothing, nor
  // those of a module that can be off while the person's settings load. This
  // hides in the app; the server checks access on its own.
  const projectExperience = useProjectExperience(current?.id)
  const homeExperience = useHomeExperience()
  const experience = current ? projectExperience : homeExperience
  const experienceRef = useRef(experience)
  useEffect(() => {
    currentRef.current = current
  }, [current])
  useEffect(() => {
    experienceRef.current = experience
  }, [experience])

  useEffect(() => {
    const prefetch = () => void loadPalette().catch(() => undefined)
    if (typeof window.requestIdleCallback === 'function') {
      const handle = window.requestIdleCallback(prefetch, { timeout: 5000 })
      return () => window.cancelIdleCallback(handle)
    }
    const timer = window.setTimeout(prefetch, 2000)
    return () => window.clearTimeout(timer)
  }, [])

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
        // Keeps the open tasks view (filters, board, open task) behind the dialog.
        if (project) go(createTaskHref(project.id, window.location))
        else openCommandMenu('Create task in')
      },
      search: () => go('/search'),
      'mark-all-read': openMarkAllRead,
      'previous-conversation': () => moveInSidebar(-1, false),
      'next-conversation': () => moveInSidebar(1, false),
      'previous-unread': () => moveInSidebar(-1, true),
      'next-unread': () => moveInSidebar(1, true),
      'go-home': () => go('/home'),
      'go-inbox': () => go('/inbox'),
      'go-my-work': () => go('/my-work'),
      'go-later': () => go('/later'),
      'go-messages': () => go('/messages'),
      'go-tasks': () => inProject('/work-items'),
      'go-board': () => inProject('/work-items?view=board'),
      'go-pages': () => inProject('/pages'),
      'go-decisions': () => inProject('/decisions'),
      'go-settings': () => {
        const project = currentRef.current
        go(project ? projectSettingsHref(project.id) : '/settings')
      },
      'go-appearance': () => go('/settings/account/appearance'),
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return
      const action = press(event)
      if (!action) return
      const module = ownerOf(shortcutClaims, action)
      if (module && moduleSwitchable(module)) {
        const experience = experienceRef.current
        if (experience.modules === 'loading' || !experience.moduleOn(module)) return
      }
      event.preventDefault()
      // One sheet: it lists the task list and board keys too, so a page's own
      // "?" listener (on window, after this one) doesn't open a second.
      if (action === 'shortcut-sheet') event.stopPropagation()
      if (action === 'command-menu' && document.querySelector('.aw-cmdk')) {
        closeCommandMenu()
        return
      }
      handlers[action]()
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [navigate])

  return state.open ? (
    <Suspense fallback={null}>
      <CommandPalette />
    </Suspense>
  ) : null
}

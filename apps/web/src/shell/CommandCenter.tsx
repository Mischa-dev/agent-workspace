import { lazy, Suspense, useEffect, useMemo, useRef } from 'react'
import { useNavigate } from 'react-router-dom'

import { useProjects } from '../projects/hooks'
import { projectSettingsHref } from '../settings/paths'
import { closeCommandMenu, openCommandMenu, openShortcutSheet, useCommandState } from './commandStore'
import { createShortcutMatcher, type ShortcutAction } from './shortcuts'
import { projectPath, useRouteProject } from './useRouteProject'

const loadPalette = () => import('./CommandPalette')
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
  useEffect(() => {
    currentRef.current = current
  }, [current])

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
        if (project) go(projectPath(project.id, '/work-items?create=task'))
        else openCommandMenu('Create task in')
      },
      search: () => go('/search'),
      'go-home': () => go('/home'),
      'go-inbox': () => go('/inbox'),
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

  return state.open ? (
    <Suspense fallback={null}>
      <CommandPalette />
    </Suspense>
  ) : null
}

import { useEffect, useSyncExternalStore } from 'react'

import type { DirectConversation } from '../direct/api'
import type { Project } from '../projects/types'

/** What needs you: Project attention plus unread DMs and groups. */
export function unreadCount(projects: Pick<Project, 'attentionCount' | 'archivedAt'>[], conversations: Pick<DirectConversation, 'unread'>[]) {
  const projectCount = projects.reduce((sum, project) => sum + (project.archivedAt ? 0 : Math.max(0, project.attentionCount)), 0)
  return projectCount + conversations.filter((conversation) => conversation.unread).length
}

let override: string | null = null
const listeners = new Set<() => void>()

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

export function usePageTitleOverride() {
  return useSyncExternalStore(subscribe, () => override, () => override)
}

/**
 * Names the tab after what's on screen, such as "#general" or a thread's
 * name, while the calling page is mounted. Pages without one get a title from
 * their address.
 */
export function usePageTitle(title: string | null | undefined) {
  useEffect(() => {
    if (!title) return undefined
    override = title
    for (const listener of listeners) listener()
    return () => {
      if (override === title) override = null
      for (const listener of listeners) listener()
    }
  }, [title])
}

const STATIC_TITLES: [RegExp, string][] = [
  [/^\/home\/?$/, 'Home'],
  [/^\/messages\/agents\/?$/, 'Agents'],
  [/^\/messages(\/.*)?$/, 'Messages'],
  [/^\/search\/?$/, 'Search'],
  [/^\/settings\/appearance\/?$/, 'Appearance'],
  [/^\/settings\/account\/?$/, 'Account settings'],
  [/^\/projects\/new\/?$/, 'Create project'],
  [/^\/dev\/ui\/?$/, 'UI kit'],
]

const PROJECT_TITLES: [RegExp, string][] = [
  [/^\/?$/, ''],
  [/^\/work-items\/?$/, 'Tasks'],
  [/^\/work-items\/.+$/, 'Task'],
  [/^\/settings\/?$/, 'Settings'],
  [/^\/launches\/?$/, 'Launches'],
  [/^\/pages(\/.*)?$/, 'Pages'],
  [/^\/decisions\/?$/, 'Decisions'],
  [/^\/conversations\/archived\/?$/, 'Archived conversations'],
  [/^\/channels\/.+$/, 'Channel'],
  [/^\/threads\/.+$/, 'Thread'],
]

export function pageTitleFor(pathname: string, search: string, projects: Pick<Project, 'id' | 'name'>[], pageOverride: string | null) {
  const projectMatch = /^\/projects\/([^/]+)(\/.*)?$/.exec(pathname)
  if (projectMatch && projectMatch[1] !== 'new') {
    const id = decodeURIComponent(projectMatch[1])
    const project = projects.find((candidate) => candidate.id === id)
    const rest = projectMatch[2] ?? ''
    const board = new URLSearchParams(search).get('view') === 'board'
    const section = pageOverride ?? (board && /^\/work-items\/?$/.test(rest) ? 'Board' : PROJECT_TITLES.find(([pattern]) => pattern.test(rest))?.[1] ?? '')
    return [section, project?.name].filter(Boolean).join(' · ')
  }
  if (pageOverride) return pageOverride
  return STATIC_TITLES.find(([pattern]) => pattern.test(pathname))?.[1] ?? ''
}

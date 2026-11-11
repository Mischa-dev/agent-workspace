import { useEffect, useSyncExternalStore } from 'react'

import type { DirectConversation } from '../direct/api'
import type { Project } from '../projects/types'

/**
 * What needs you, for the tab title and the app badge: the inbox's unread
 * count (which covers every project, DMs and reminders) plus DMs and groups
 * with unread messages.
 */
export function unreadCount(inboxUnread: number, conversations: Pick<DirectConversation, 'unread'>[]) {
  return Math.max(0, inboxUnread) + conversations.filter((conversation) => conversation.unread).length
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
  [/^\/inbox\/?$/, 'Inbox'],
  [/^\/my-work\/?$/, 'My work'],
  [/^\/later\/?$/, 'Later'],
  [/^\/runs\/?$/, 'Runs'],
  [/^\/runs\/.+$/, 'Run'],
  [/^\/whiteboards\/?$/, 'Whiteboards'],
  [/^\/whiteboards\/.+$/, 'Whiteboard'],
  [/^\/messages\/agents\/?$/, 'Agents'],
  [/^\/messages(\/.*)?$/, 'Messages'],
  [/^\/search\/?$/, 'Search'],
  [/^\/welcome\/?$/, 'Setup'],
  [/^\/new-task\/?$/, 'New task'],
  [/^\/share\/?$/, 'Share'],
  [/^\/settings\/account\/appearance\/?$/, 'Appearance'],
  [/^\/settings(\/.*)?$/, 'Settings'],
  [/^\/projects\/new\/?$/, 'New project'],
  [/^\/projects\/?$/, 'Projects'],
  [/^\/design-system\/?$/, 'Design system'],
]

const PROJECT_TITLES: [RegExp, string][] = [
  [/^\/?$/, ''],
  [/^\/work-items\/?$/, 'Tasks'],
  [/^\/work-items\/.+$/, 'Task'],
  [/^\/milestones\/?$/, 'Milestones'],
  [/^\/timeline\/?$/, 'Timeline'],
  [/^\/next\/?$/, 'Planning'],
  [/^\/suggestions\/?$/, 'Suggestions'],
  [/^\/graph\/?$/, 'Dependency graph'],
  [/^\/plan-import\/?$/, 'Import a plan'],
  [/^\/settings\/?$/, 'Settings'],
  [/^\/launches\/?$/, 'Runs'],
  [/^\/pages(\/.*)?$/, 'Pages'],
  [/^\/decisions\/?$/, 'Decisions'],
  [/^\/collections(\/.*)?$/, 'Collections'],
  [/^\/conversations\/archived\/?$/, 'Archived conversations'],
  [/^\/channels\/.+$/, 'Channel'],
  [/^\/threads\/.+$/, 'Thread'],
]

/** The name of a page outside any project ("Search", "Settings"), or empty. */
export function staticPageTitle(pathname: string) {
  return STATIC_TITLES.find(([pattern]) => pattern.test(pathname))?.[1] ?? ''
}

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
  return staticPageTitle(pathname)
}

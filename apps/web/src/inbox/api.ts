import { useInfiniteQuery, useMutation, useQueryClient, type InfiniteData, type QueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { queryKeys } from '../api/queryKeys'

export const inboxKeys = {
  root: ['inbox'] as const,
  list: (view: InboxView, category: InboxCategory) => ['inbox', 'list', view, category] as const,
}

export function invalidateInbox(queryClient: QueryClient) {
  void queryClient.invalidateQueries({ queryKey: inboxKeys.root })
}

export type InboxView = 'inbox' | 'unread' | 'snoozed' | 'archived'
export type InboxCategory = 'all' | 'mentions' | 'tasks' | 'runs' | 'pull_requests' | 'usage'
export type InboxAction = 'read' | 'unread' | 'archive' | 'restore' | 'snooze' | 'unsnooze'

export type InboxApproval = {
  id: string
  kind: 'approval' | 'question'
  status: string
  options: Array<{ id: string; label: string }>
}

export type InboxNotification = {
  id: string
  kind: string
  projectId: string
  projectName: string
  subject: { kind: string; id: string; title: string }
  title: string
  preview: string | null
  actor: { id: string; name: string } | null
  count: number
  read: boolean
  archived: boolean
  snoozedUntil: string | null
  at: string
  href: string
  /** Capacity alerts: the ready tasks that could use the capacity. */
  launchHref: string | null
  approval: InboxApproval | null
}

export type InboxPage = {
  notifications: InboxNotification[]
  nextCursor: string | null
  unreadCount: number
}

type WireRecord = Record<string, unknown>

function asRecord(value: unknown): WireRecord {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as WireRecord) : {}
}

function text(value: unknown): string | null {
  return typeof value === 'string' ? value : null
}

/** A safe in-app path, or an https link (pull requests). Anything else opens the project. */
export function safeHref(href: string, projectId: string): string {
  if (href.startsWith('/') && !href.startsWith('//')) return href
  if (/^https:\/\/[^/\s]+\//.test(href)) return href
  return `/projects/${encodeURIComponent(projectId)}`
}

function inAppPath(href: string | null) {
  return href && href.startsWith('/') && !href.startsWith('//') ? href : null
}

export function isExternalHref(href: string) {
  return href.startsWith('https://')
}

function normalizeApproval(value: unknown): InboxApproval | null {
  const record = asRecord(value)
  const id = text(record.id)
  const status = text(record.status)
  if (!id || !status || (record.kind !== 'approval' && record.kind !== 'question')) return null
  const options = Array.isArray(record.options)
    ? record.options
        .map(asRecord)
        .filter((option) => typeof option.id === 'string' && typeof option.label === 'string')
        .map((option) => ({ id: option.id as string, label: option.label as string }))
    : []
  return { id, kind: record.kind, status, options }
}

export function normalizeNotification(value: unknown): InboxNotification | null {
  const record = asRecord(value)
  const subject = asRecord(record.subject)
  const actor = asRecord(record.actor)
  const id = text(record.id)
  const kind = text(record.kind)
  const projectId = text(record.projectId)
  const title = text(record.title)
  const at = text(record.at)
  if (!id || !kind || !projectId || !title || !at) return null
  return {
    id,
    kind,
    projectId,
    projectName: text(record.projectName) ?? '',
    subject: {
      kind: text(subject.kind) ?? 'project',
      id: text(subject.id) ?? projectId,
      title: text(subject.title) ?? '',
    },
    title,
    preview: text(record.preview),
    actor: text(actor.id) && text(actor.name) ? { id: actor.id as string, name: actor.name as string } : null,
    count: typeof record.count === 'number' && record.count > 0 ? record.count : 1,
    read: record.read === true,
    archived: record.archived === true,
    snoozedUntil: text(record.snoozedUntil),
    at,
    href: safeHref(text(record.href) ?? '', projectId),
    launchHref: inAppPath(text(record.launchHref)),
    approval: normalizeApproval(record.approval),
  }
}

export function normalizePage(value: unknown): InboxPage {
  const record = asRecord(value)
  return {
    notifications: Array.isArray(record.notifications)
      ? record.notifications
          .map(normalizeNotification)
          .filter((item): item is InboxNotification => item !== null)
      : [],
    nextCursor: text(record.nextCursor),
    unreadCount: typeof record.unreadCount === 'number' ? record.unreadCount : 0,
  }
}

export function useInbox(view: InboxView, category: InboxCategory) {
  return useInfiniteQuery({
    queryKey: inboxKeys.list(view, category),
    initialPageParam: null as string | null,
    queryFn: async ({ pageParam }) => {
      const search = new URLSearchParams({ view, category, limit: '50' })
      if (pageParam) search.set('cursor', pageParam)
      return normalizePage(await requestJson<unknown>(`/api/v1/inbox?${search}`))
    },
    getNextPageParam: (page) => page.nextCursor,
  })
}

export type InboxUpdate =
  | { action: InboxAction; ids: string[]; snoozeUntil?: string }
  | { action: 'read' | 'archive'; all: true }

/** What the update does to a cached row: changed, or gone from this view. */
export function applyUpdate(
  notification: InboxNotification,
  view: InboxView,
  update: InboxUpdate,
): InboxNotification | null {
  if ('ids' in update && !update.ids.includes(notification.id)) return notification
  if ('all' in update && view !== 'inbox' && view !== 'unread') return notification
  switch (update.action) {
    case 'read':
      return view === 'unread' ? null : { ...notification, read: true }
    case 'unread':
      return { ...notification, read: false }
    case 'archive':
      return view === 'archived' ? { ...notification, archived: true, read: true } : null
    case 'restore':
      return view === 'archived' ? null : { ...notification, archived: false }
    case 'snooze':
      return view === 'snoozed' ? notification : null
    case 'unsnooze':
      return view === 'snoozed' ? null : notification
  }
}

type InboxCache = InfiniteData<InboxPage, string | null>

export function useUpdateInbox(view: InboxView, category: InboxCategory) {
  const queryClient = useQueryClient()
  const key = inboxKeys.list(view, category)
  return useMutation({
    mutationFn: (update: InboxUpdate) =>
      requestJson<unknown>('/api/v1/inbox/update', { method: 'POST', json: update }),
    onMutate: async (update) => {
      await queryClient.cancelQueries({ queryKey: key })
      const previous = queryClient.getQueryData<InboxCache>(key)
      if (previous) {
        queryClient.setQueryData<InboxCache>(key, {
          ...previous,
          pages: previous.pages.map((page) => ({
            ...page,
            notifications: page.notifications
              .map((item) => applyUpdate(item, view, update))
              .filter((item): item is InboxNotification => item !== null),
          })),
        })
      }
      return { previous }
    },
    onError: (_error, _update, context) => {
      if (context?.previous) queryClient.setQueryData(key, context.previous)
    },
    onSettled: () => {
      invalidateInbox(queryClient)
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects })
    },
  })
}

export function respondToApproval(approvalId: string, decision: 'approve' | 'deny') {
  return requestJson<unknown>(`/api/v1/agent-approvals/${encodeURIComponent(approvalId)}/respond`, {
    method: 'POST',
    idempotencyKey: `approval-${approvalId}-${decision}`,
    json: { decision },
  })
}

/** Snooze choices, in the viewer's own time zone. */
export function snoozeChoices(now = new Date()): Array<{ id: string; label: string; until: Date }> {
  const laterToday = new Date(now.getTime() + 3 * 60 * 60 * 1000)
  const tomorrow = new Date(now)
  tomorrow.setDate(now.getDate() + 1)
  tomorrow.setHours(9, 0, 0, 0)
  const nextWeek = new Date(now)
  const daysToMonday = ((8 - now.getDay()) % 7) || 7
  nextWeek.setDate(now.getDate() + daysToMonday)
  nextWeek.setHours(9, 0, 0, 0)
  return [
    { id: 'later', label: 'Later today', until: laterToday },
    { id: 'tomorrow', label: 'Tomorrow morning', until: tomorrow },
    { id: 'next-week', label: 'Next week', until: nextWeek },
  ]
}

/** Notifications grouped by what they're about, keeping their order. */
export function groupBySubject(notifications: InboxNotification[]) {
  const groups: Array<{ key: string; title: string; projectName: string; items: InboxNotification[] }> = []
  const byKey = new Map<string, (typeof groups)[number]>()
  for (const notification of notifications) {
    const key = `${notification.subject.kind}:${notification.subject.id}`
    let group = byKey.get(key)
    if (!group) {
      group = { key, title: notification.subject.title, projectName: notification.projectName, items: [] }
      byKey.set(key, group)
      groups.push(group)
    }
    group.items.push(notification)
  }
  return groups
}

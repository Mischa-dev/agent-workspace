import { useInfiniteQuery, useMutation, useQueryClient, type InfiniteData, type QueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { queryKeys } from '../api/queryKeys'
import { whenChoices } from '../later/when'
import { actorKind, type ActorKind } from '../ui/identityModel'

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
  /** Who caused it; agents keep their square mark and Agent tag. */
  actor: { id: string; name: string; kind: ActorKind } | null
  count: number
  read: boolean
  archived: boolean
  snoozedUntil: string | null
  at: string
  href: string
  /** Capacity alerts: the ready tasks that could use the capacity. */
  launchHref: string | null
  /** Capacity alerts: the suggested batch, which Launch starts in one tap. */
  batch: { accountId: string; size: number; windowKey: string | null } | null
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

/** Agents read as agents here, whichever way they run. */
function inboxActorKind(kind: string | null): ActorKind {
  const mapped = actorKind(kind ?? 'human')
  return mapped === 'worker' ? 'agent' : mapped
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
    actor:
      text(actor.id) && text(actor.name)
        ? { id: actor.id as string, name: actor.name as string, kind: inboxActorKind(text(actor.kind)) }
        : null,
    count: typeof record.count === 'number' && record.count > 0 ? record.count : 1,
    read: record.read === true,
    archived: record.archived === true,
    snoozedUntil: text(record.snoozedUntil),
    at,
    href: safeHref(text(record.href) ?? '', projectId),
    launchHref: inAppPath(text(record.launchHref)),
    batch: normalizeBatch(record.batch),
    approval: normalizeApproval(record.approval),
  }
}

function normalizeBatch(value: unknown): InboxNotification['batch'] {
  const record = asRecord(value)
  const accountId = text(record.accountId)
  const size = typeof record.size === 'number' && record.size > 0 ? record.size : 0
  return accountId && size ? { accountId, size, windowKey: text(record.windowKey) } : null
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

/** What the server changed: the notifications that moved, for Undo. */
export type InboxUpdateResult = { updated: string[]; unreadCount: number }

/**
 * What the update does to a cached row: changed, or gone from this view. In
 * Unread, a row you just read stays (marked read) so you can act on it; only
 * "mark all read" empties the view at once.
 */
export function applyUpdate(
  notification: InboxNotification,
  view: InboxView,
  update: InboxUpdate,
): InboxNotification | null {
  if ('ids' in update && !update.ids.includes(notification.id)) return notification
  if ('all' in update && view !== 'inbox' && view !== 'unread') return notification
  switch (update.action) {
    case 'read':
      return view === 'unread' && 'all' in update ? null : { ...notification, read: true }
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
    mutationFn: (update: InboxUpdate) => postUpdate(update),
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

function readResult(value: unknown): InboxUpdateResult {
  const record = asRecord(value)
  return {
    updated: Array.isArray(record.updated) ? record.updated.filter((id): id is string => typeof id === 'string') : [],
    unreadCount: typeof record.unreadCount === 'number' ? record.unreadCount : 0,
  }
}

/** The server takes at most this many ids in one change. */
const MAX_UPDATE_IDS = 200

/** One change, split into requests the server accepts when it names many ids. */
export async function postUpdate(update: InboxUpdate): Promise<InboxUpdateResult> {
  if (!('ids' in update) || update.ids.length <= MAX_UPDATE_IDS) {
    return readResult(await requestJson<unknown>('/api/v1/inbox/update', { method: 'POST', json: update }))
  }
  const result: InboxUpdateResult = { updated: [], unreadCount: 0 }
  for (let start = 0; start < update.ids.length; start += MAX_UPDATE_IDS) {
    const part = await postUpdate({ ...update, ids: update.ids.slice(start, start + MAX_UPDATE_IDS) })
    result.updated.push(...part.updated)
    result.unreadCount = part.unreadCount
  }
  return result
}

/** The most unread notifications one "mark read" gathers in a category. */
const MAX_CATEGORY_READ = 1_000

/**
 * The unread notifications of one kind, every page of them, so "mark all
 * read" under a filter marks exactly what the filter shows.
 */
export async function unreadIds(category: InboxCategory): Promise<string[]> {
  const ids: string[] = []
  let cursor: string | null = null
  do {
    const search = new URLSearchParams({ view: 'unread', category, limit: '100' })
    if (cursor) search.set('cursor', cursor)
    const page = normalizePage(await requestJson<unknown>(`/api/v1/inbox?${search}`))
    ids.push(...page.notifications.map((item) => item.id))
    cursor = page.nextCursor
  } while (cursor && ids.length < MAX_CATEGORY_READ)
  return ids
}

export function respondToApproval(approvalId: string, decision: 'approve' | 'deny') {
  return requestJson<unknown>(`/api/v1/agent-approvals/${encodeURIComponent(approvalId)}/respond`, {
    method: 'POST',
    idempotencyKey: `approval-${approvalId}-${decision}`,
    json: { decision },
  })
}

/**
 * Snooze choices, in the viewer's own time zone and by the same rules as
 * Later's reminders: "Later today" only while the day has some left, and the
 * morning after the night's sleep.
 */
export function snoozeChoices(now = new Date()): Array<{ id: string; label: string; until: Date }> {
  const quick = whenChoices(now)
  const pick = (id: string) => quick.find((choice) => choice.id === id)!
  const hour = now.getHours()
  return [
    ...(hour >= 5 && hour < 18 ? [{ id: 'later', label: 'Later today', until: new Date(now.getTime() + 3 * 60 * 60 * 1000) }] : []),
    { id: 'tomorrow', label: pick('tomorrow').label, until: pick('tomorrow').at },
    { id: 'next-week', label: pick('next-week').label, until: pick('next-week').at },
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

/**
 * Kinds that ask something of you: someone mentioned or messaged you, gave
 * you a task or a review, an approval or a waiting run is yours to answer,
 * or something of yours failed. Everything else is for your information.
 */
const NEEDS_YOU_KINDS = new Set([
  'mention',
  'comment_mention',
  'conversation_message',
  'assigned',
  'approval_requested',
  'review_requested',
  'run_waiting',
  'run_failed',
  'checks_failed',
  'scheduled_send_failed',
  'reminder',
])

/** Whether a notification asks something of you; an approval someone already answered no longer does. */
export function needsYou(notification: InboxNotification) {
  if (notification.approval && notification.approval.status !== 'pending') return false
  return NEEDS_YOU_KINDS.has(notification.kind)
}

export type InboxSection = {
  key: 'needs' | 'fyi'
  title: string
  groups: ReturnType<typeof groupBySubject>
  items: InboxNotification[]
}

/** The inbox in two parts, each grouped by subject: what needs you first, then the rest. */
export function sectionsByNeed(notifications: InboxNotification[]): InboxSection[] {
  const needs = notifications.filter(needsYou)
  const fyi = notifications.filter((notification) => !needsYou(notification))
  return [
    { key: 'needs', title: 'Needs you', groups: groupBySubject(needs), items: needs },
    { key: 'fyi', title: 'FYI', groups: groupBySubject(fyi), items: fyi },
  ]
}

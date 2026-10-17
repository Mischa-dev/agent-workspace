import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import type { ConversationTimelineEntry, MessageContentV1 } from '../conversations/types'
import { normalizeTimelineEntry } from '../conversations/api'

export const laterKeys = {
  root: ['later'] as const,
  list: (view: LaterView) => ['later', 'list', view] as const,
  scheduled: (conversationId: string | null) => ['later', 'scheduled', conversationId ?? 'all'] as const,
  level: (conversationId: string) => ['later', 'level', conversationId] as const,
}

/** Saved items, scheduled messages and levels all change together often enough to refresh as one. */
export function invalidateLater(queryClient: QueryClient) {
  void queryClient.invalidateQueries({ queryKey: laterKeys.root })
}

export type LaterView = 'open' | 'done'

export type SavedItem = {
  id: string
  kind: 'message' | 'work_item' | 'reminder'
  note: string
  remindAt: string | null
  done: boolean
  doneAt: string | null
  createdAt: string
  projectId: string
  projectName: string
  href: string
  message: {
    id: string
    conversationId: string
    place: string
    authorName: string
    preview: string
    removed: boolean
    createdAt: string
  } | null
  workItem: { id: string; key: string; title: string; statusCategory: string } | null
}

export type LaterPage = { items: SavedItem[]; openCount: number }

export type ScheduledMessage = {
  id: string
  conversationId: string
  place: string
  projectId: string
  projectName: string
  href: string
  content: MessageContentV1
  plainText: string
  sendAt: string
  status: 'scheduled' | 'sent' | 'failed' | 'canceled'
  failure: string | null
  messageHref: string | null
  createdAt: string
}

export type NotificationLevel = 'all' | 'mentions' | 'nothing'

export type ConversationLevel = {
  conversationId: string
  projectId: string
  place: string
  level: NotificationLevel
  own: boolean
}

type WireRecord = Record<string, unknown>

function asRecord(value: unknown): WireRecord {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as WireRecord) : {}
}

function str(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : fallback
}

function maybe(value: unknown): string | null {
  return typeof value === 'string' ? value : null
}

/** Only in-app paths are followed; anything else opens the Later page. */
function inApp(value: unknown): string {
  const href = str(value)
  return href.startsWith('/') && !href.startsWith('//') ? href : '/later'
}

export function normalizeSavedItem(value: unknown): SavedItem | null {
  const item = asRecord(value)
  const kind = item.kind
  if (typeof item.id !== 'string' || (kind !== 'message' && kind !== 'work_item' && kind !== 'reminder')) return null
  const message = asRecord(item.message)
  const work = asRecord(item.workItem)
  return {
    id: item.id,
    kind,
    note: str(item.note).slice(0, 1_000),
    remindAt: maybe(item.remindAt),
    done: item.done === true,
    doneAt: maybe(item.doneAt),
    createdAt: str(item.createdAt),
    projectId: str(item.projectId),
    projectName: str(item.projectName),
    href: inApp(item.href),
    message:
      typeof message.id === 'string'
        ? {
            id: message.id,
            conversationId: str(message.conversationId),
            place: str(message.place),
            authorName: str(message.authorName),
            preview: str(message.preview).slice(0, 400),
            removed: message.removed === true,
            createdAt: str(message.createdAt),
          }
        : null,
    workItem:
      typeof work.id === 'string'
        ? { id: work.id, key: str(work.key), title: str(work.title), statusCategory: str(work.statusCategory) }
        : null,
  }
}

function normalizeScheduled(value: unknown): ScheduledMessage | null {
  const item = asRecord(value)
  const status = item.status
  if (
    typeof item.id !== 'string' ||
    (status !== 'scheduled' && status !== 'sent' && status !== 'failed' && status !== 'canceled')
  ) {
    return null
  }
  return {
    id: item.id,
    conversationId: str(item.conversationId),
    place: str(item.place),
    projectId: str(item.projectId),
    projectName: str(item.projectName),
    href: inApp(item.href),
    content: item.content as MessageContentV1,
    plainText: str(item.plainText),
    sendAt: str(item.sendAt),
    status,
    failure: maybe(item.failure),
    messageHref: typeof item.messageHref === 'string' ? inApp(item.messageHref) : null,
    createdAt: str(item.createdAt),
  }
}

function normalizeLevel(value: unknown): ConversationLevel {
  const level = asRecord(value)
  const name = level.level
  return {
    conversationId: str(level.conversationId),
    projectId: str(level.projectId),
    place: str(level.place),
    level: name === 'all' || name === 'nothing' ? name : 'mentions',
    own: level.own === true,
  }
}

export async function fetchLater(view: LaterView): Promise<LaterPage> {
  const page = asRecord(await requestJson<unknown>(`/api/v1/later?view=${view}`))
  return {
    items: Array.isArray(page.items)
      ? page.items.map(normalizeSavedItem).filter((item): item is SavedItem => item !== null)
      : [],
    openCount: typeof page.openCount === 'number' ? page.openCount : 0,
  }
}

export function useLater(view: LaterView) {
  return useQuery({ queryKey: laterKeys.list(view), queryFn: () => fetchLater(view), staleTime: 15_000 })
}

export type SaveInput = {
  messageId?: string
  workItemId?: string
  note?: string
  remindAt?: string | null
}

export async function saveForLater(input: SaveInput): Promise<SavedItem> {
  const body = asRecord(await requestJson<unknown>('/api/v1/later', { method: 'POST', json: input }))
  const item = normalizeSavedItem(body.item)
  if (!item) throw new Error('The saved item came back unreadable.')
  return item
}

export async function changeSaved(
  id: string,
  change: { remindAt?: string | null; note?: string; done?: boolean },
): Promise<SavedItem> {
  const body = asRecord(
    await requestJson<unknown>(`/api/v1/later/${encodeURIComponent(id)}`, { method: 'PATCH', json: change }),
  )
  const item = normalizeSavedItem(body.item)
  if (!item) throw new Error('The saved item came back unreadable.')
  return item
}

export async function removeSaved(id: string) {
  await requestJson<unknown>(`/api/v1/later/${encodeURIComponent(id)}`, { method: 'DELETE' })
}

export async function fetchScheduled(conversationId: string | null): Promise<ScheduledMessage[]> {
  const query = conversationId ? `?conversationId=${encodeURIComponent(conversationId)}` : ''
  const body = asRecord(await requestJson<unknown>(`/api/v1/scheduled-messages${query}`))
  return Array.isArray(body.scheduledMessages)
    ? body.scheduledMessages.map(normalizeScheduled).filter((item): item is ScheduledMessage => item !== null)
    : []
}

export function useScheduled(conversationId: string | null, enabled = true) {
  return useQuery({
    queryKey: laterKeys.scheduled(conversationId),
    queryFn: () => fetchScheduled(conversationId),
    enabled,
    staleTime: 15_000,
  })
}

function scheduledFrom(value: unknown) {
  const item = normalizeScheduled(asRecord(value).scheduledMessage)
  if (!item) throw new Error('The scheduled message came back unreadable.')
  return item
}

export async function scheduleMessage(conversationId: string, content: MessageContentV1, sendAt: string) {
  return scheduledFrom(
    await requestJson<unknown>(`/api/v1/conversations/${encodeURIComponent(conversationId)}/scheduled-messages`, {
      method: 'POST',
      json: { content, sendAt },
    }),
  )
}

export async function updateScheduled(id: string, change: { content?: MessageContentV1; sendAt?: string }) {
  return scheduledFrom(
    await requestJson<unknown>(`/api/v1/scheduled-messages/${encodeURIComponent(id)}`, { method: 'PATCH', json: change }),
  )
}

export async function sendScheduledNow(id: string) {
  return scheduledFrom(
    await requestJson<unknown>(`/api/v1/scheduled-messages/${encodeURIComponent(id)}/send-now`, {
      method: 'POST',
      json: {},
    }),
  )
}

export async function cancelScheduled(id: string) {
  await requestJson<unknown>(`/api/v1/scheduled-messages/${encodeURIComponent(id)}`, { method: 'DELETE' })
}

export async function forwardMessage(input: {
  messageId: string
  conversationId: string
  text?: string
  idempotencyKey: string
}): Promise<ConversationTimelineEntry | null> {
  const body = asRecord(
    await requestJson<unknown>(`/api/v1/messages/${encodeURIComponent(input.messageId)}/forward`, {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      json: { conversationId: input.conversationId, ...(input.text?.trim() ? { text: input.text.trim() } : {}) },
    }),
  )
  return normalizeTimelineEntry(body.entry)
}

export async function fetchLevel(conversationId: string): Promise<ConversationLevel> {
  return normalizeLevel(
    await requestJson<unknown>(`/api/v1/conversations/${encodeURIComponent(conversationId)}/notification-level`),
  )
}

export function useConversationLevel(conversationId: string, enabled = true) {
  return useQuery({
    queryKey: laterKeys.level(conversationId),
    queryFn: () => fetchLevel(conversationId),
    enabled: enabled && Boolean(conversationId),
    staleTime: 60_000,
  })
}

/** Null clears it: back to the default, or a thread following its channel. */
export async function setLevel(conversationId: string, level: NotificationLevel | null) {
  return normalizeLevel(
    await requestJson<unknown>(`/api/v1/conversations/${encodeURIComponent(conversationId)}/notification-level`, {
      method: 'PUT',
      json: { level },
    }),
  )
}

export function useLaterRefresh() {
  const queryClient = useQueryClient()
  return () => invalidateLater(queryClient)
}

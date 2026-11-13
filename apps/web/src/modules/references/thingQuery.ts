import { requestJson } from '../../api/client'
import { isApiError } from '../../api/errors'
import type { Thing, ThingLink } from '../contract'

/**
 * One thing and its links, as `GET /api/v1/things/{kind}/{id}` answers. The
 * key is the one embeds use, so they share it and realtime refreshes both.
 */
export function thingQuery(kind: string, id: string) {
  return {
    queryKey: ['things', kind, id],
    queryFn: () => requestJson<{ thing: Thing; links: ThingLink[] }>(`/api/v1/things/${encodeURIComponent(kind)}/${encodeURIComponent(id)}`),
    retry: (count: number, error: unknown) => !(isApiError(error) && error.status < 500) && count < 2,
    staleTime: 30_000,
  }
}

const ICONS = {
  task: 'checklist',
  page: 'list',
  decision: 'check',
  whiteboard: 'whiteboard',
  thread: 'messages',
  channel: 'hash',
  conversation: 'message',
  project: 'board',
} as const

export function thingIcon(kind: string) {
  return ICONS[kind as keyof typeof ICONS] ?? 'link'
}

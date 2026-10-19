import { requestJson } from '../api/client'

export type PageSummary = {
  id: string
  title: string
  isGuide: boolean
  /** Set when the page is a skill agents load: when to use it. */
  skillDescription: string | null
  version: number
  updatedAt: string
  updatedBy: string | null
  stale: boolean
  excerpt: string
}

export type PageRevision = {
  version: number
  title: string
  author: string | null
  via: string | null
  note: string | null
  createdAt: string
}

export type Page = {
  id: string
  projectId: string
  title: string
  body: string
  isGuide: boolean
  skillDescription: string | null
  version: number
  createdAt: string
  updatedAt: string
  updatedBy: string | null
  archived: boolean
  stale: boolean
  revisions: PageRevision[]
}

export type PageInput = {
  title?: string
  body?: string
  isGuide?: boolean
  /** One line saying when agents should use it; an empty string stops it being a skill. */
  skillDescription?: string
  expectedVersion?: number
  restoreVersion?: number
  archived?: boolean
  note?: string
}

export type Decision = {
  id: string
  projectId: string
  title: string
  detail: string
  status: 'active' | 'superseded'
  supersededByDecisionId: string | null
  sourceConversationId: string | null
  sourceConversationName: string | null
  /** channel, thread, dm or group; missing from servers older than this field. */
  sourceConversationKind?: string | null
  sourceMessageId: string | null
  decidedBy: string
  via: string | null
  createdAt: string
}

export type Pin = {
  kind: 'message' | 'page' | 'decision'
  id: string
  title: string
  text: string
  pinnedAt: string
}

const project = (projectId: string) => `/api/v1/projects/${encodeURIComponent(projectId)}`

// Project first, so a project's refresh and reset reach them.
export const memoryKeys = {
  pages: (projectId: string) => ['project', projectId, 'memory', 'pages'] as const,
  page: (projectId: string, pageId: string) => ['project', projectId, 'memory', 'page', pageId] as const,
  decisions: (projectId: string) => ['project', projectId, 'memory', 'decisions'] as const,
  pins: (projectId: string, conversationId: string) =>
    ['project', projectId, 'memory', 'pins', conversationId] as const,
}

export function listPages(projectId: string) {
  return requestJson<{ pages: PageSummary[]; canEdit: boolean }>(`${project(projectId)}/pages`)
}

export function createPage(projectId: string, input: PageInput) {
  return requestJson<Page>(`${project(projectId)}/pages`, { method: 'POST', json: input })
}

export function getPage(pageId: string) {
  return requestJson<Page>(`/api/v1/pages/${encodeURIComponent(pageId)}`)
}

export function getPageVersion(pageId: string, version: number) {
  return requestJson<PageRevision & { body: string }>(
    `/api/v1/pages/${encodeURIComponent(pageId)}/revisions/${version}`,
  )
}

export function updatePage(pageId: string, input: PageInput) {
  return requestJson<Page>(`/api/v1/pages/${encodeURIComponent(pageId)}`, { method: 'PATCH', json: input })
}

export function listDecisions(projectId: string, includeSuperseded = false) {
  const query = includeSuperseded ? '?includeSuperseded=true' : ''
  return requestJson<{ decisions: Decision[]; canRecord: boolean }>(`${project(projectId)}/decisions${query}`)
}

export function recordDecision(
  projectId: string,
  input: { title: string; detail?: string; sourceMessageId?: string; sourceConversationId?: string; supersedes?: string },
) {
  return requestJson<Decision>(`${project(projectId)}/decisions`, { method: 'POST', json: input })
}

export function listPins(conversationId: string) {
  return requestJson<{ pins: Pin[] }>(`/api/v1/conversations/${encodeURIComponent(conversationId)}/pins`)
}

export function pin(conversationId: string, kind: Pin['kind'], targetId: string) {
  return requestJson<{ pins: Pin[] }>(`/api/v1/conversations/${encodeURIComponent(conversationId)}/pins`, {
    method: 'POST',
    json: { kind, targetId },
  })
}

export function unpin(conversationId: string, kind: Pin['kind'], targetId: string) {
  return requestJson<{ pins: Pin[] }>(
    `/api/v1/conversations/${encodeURIComponent(conversationId)}/pins/${kind}/${encodeURIComponent(targetId)}`,
    { method: 'DELETE' },
  )
}

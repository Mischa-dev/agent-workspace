import { apiUrl, requestJson } from '../api/client'
import { callOp, newRequestId } from '../api/ops'

export type PageSummary = {
  id: string
  title: string
  isGuide: boolean
  /** A template: offered in "New page" as something to start from. */
  isTemplate: boolean
  /** Set when the page is a skill agents load: when to use it. */
  skillDescription: string | null
  version: number
  updatedAt: string
  updatedBy: string | null
  stale: boolean
  excerpt: string
  /** The page it sits inside, if any. */
  parentPageId: string | null
  /** Its place among its siblings. */
  position: number
}

/** A page named in a breadcrumb or a list of the pages inside one. */
export type PageCrumb = { id: string; title: string }

export type PageRevision = {
  version: number
  title: string
  author: string | null
  via: string | null
  note: string | null
  createdAt: string
  /** The agent session that wrote it: what ran it, and where, when the viewer can see that conversation. */
  sessionHarness?: string
  sessionRunId?: string
  sessionConversationId?: string
  sessionConversationName?: string
  sessionConversationKind?: string
}

export type Page = {
  id: string
  projectId: string
  title: string
  body: string
  isGuide: boolean
  isTemplate: boolean
  skillDescription: string | null
  version: number
  createdAt: string
  updatedAt: string
  updatedBy: string | null
  archived: boolean
  stale: boolean
  revisions: PageRevision[]
  /** The page it sits inside, if any. */
  parentPageId: string | null
  /** The pages it sits inside, outermost first. */
  ancestors: PageCrumb[]
  /** The pages inside it, in order. */
  children: PageCrumb[]
}

export type PageInput = {
  title?: string
  body?: string
  isGuide?: boolean
  isTemplate?: boolean
  /** Start the new page from this template (a template page's id or a built-in's name). */
  fromTemplateId?: string
  /** One line saying when agents should use it; an empty string stops it being a skill. */
  skillDescription?: string
  /** Put it inside this page of its project, or back at the top (null). */
  parentPageId?: string | null
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
}

const project = (projectId: string) => `/api/v1/projects/${encodeURIComponent(projectId)}`

// Project first, so a project's refresh and reset reach them.
export { memoryKeys } from './keys'

/** The workspace's own pages live in its hidden space; this finds (or makes) it. */
export function workspacePagesSpace(organizationId: string) {
  return requestJson<{ projectId: string }>(`/api/v1/organizations/${encodeURIComponent(organizationId)}/pages-space`)
}

export function listPages(projectId: string) {
  return requestJson<{ pages: PageSummary[]; canEdit: boolean; canSuggest?: boolean }>(`${project(projectId)}/pages`)
}

/** Makes a page (`page_create`) and reads it back. */
export async function createPage(projectId: string, input: PageInput, requestId: string = newRequestId()) {
  const created = await callOp('page_create', { projectId, ...input }, { requestId })
  return getPage(created.id)
}

/** A page made from a file: what the file held that a page can't, left out. */
export type PageImport = { page: Page; setAside: string[] }

/** The file types a page can be made from. */
export const PAGE_IMPORT_ACCEPT = '.docx,.md,.markdown,.txt'

/** Makes a page from a Word document (.docx) or a Markdown file. */
export function importPage(projectId: string, file: File) {
  return requestJson<PageImport>(`${project(projectId)}/pages/import?name=${encodeURIComponent(file.name)}`, {
    method: 'POST',
    body: file,
    headers: { 'content-type': file.type || 'application/octet-stream' },
  })
}

/** Where a page downloads as Markdown, a Word document or a PDF. */
export function pageExportHref(pageId: string, format: 'markdown' | 'docx' | 'pdf') {
  return apiUrl(`/api/v1/pages/${encodeURIComponent(pageId)}/export?format=${format}`)
}

export function getPage(pageId: string) {
  return requestJson<Page>(`/api/v1/pages/${encodeURIComponent(pageId)}`)
}

export function getPageVersion(pageId: string, version: number) {
  return requestJson<PageRevision & { body: string }>(
    `/api/v1/pages/${encodeURIComponent(pageId)}/revisions/${version}`,
  )
}

/** A save as it landed; `suggestionId` when your role only suggests, so the text waits for someone who edits. */
export type SavedPage = Page & { suggestionId?: string }

/** Saves a change (`page_update`) and reads the page back. */
export async function updatePage(pageId: string, input: PageInput): Promise<SavedPage> {
  const saved = await callOp('page_update', { pageId, ...input }, { requestId: newRequestId() })
  const page = await getPage(pageId)
  return saved.suggestion ? { ...page, suggestionId: saved.suggestion.suggestionId } : page
}

export function listDecisions(projectId: string, includeSuperseded = false) {
  const query = includeSuperseded ? '?includeSuperseded=true' : ''
  return requestJson<{ decisions: Decision[]; canRecord: boolean }>(`${project(projectId)}/decisions${query}`)
}

/** Records a decision through its operation (K2.5); a retry of the same call is answered once. */
export function recordDecision(
  projectId: string,
  input: { title: string; detail?: string; sourceMessageId?: string; sourceConversationId?: string; supersedes?: string; saySo?: boolean },
  requestId: string = newRequestId(),
) {
  return callOp('decision_record', { spaceId: projectId, ...input }, { requestId }).then((decision) => decision as Decision)
}

export function listPins(conversationId: string) {
  return requestJson<{ pins: Pin[] }>(`/api/v1/conversations/${encodeURIComponent(conversationId)}/pins`)
}

/** Pin or unpin a message, page or decision in a conversation (`pin_set`). */
export async function setPinned(conversationId: string, kind: Pin['kind'], targetId: string, pinned: boolean) {
  const target = kind === 'message' ? { messageId: targetId } : kind === 'page' ? { pageId: targetId } : { decisionId: targetId }
  const answer = await callOp('pin_set', { conversationId, ...target, pinned }, { requestId: newRequestId() })
  return { pins: answer.pins as Pin[] }
}

export function pin(conversationId: string, kind: Pin['kind'], targetId: string) {
  return setPinned(conversationId, kind, targetId, true)
}

export function unpin(conversationId: string, kind: Pin['kind'], targetId: string) {
  return setPinned(conversationId, kind, targetId, false)
}

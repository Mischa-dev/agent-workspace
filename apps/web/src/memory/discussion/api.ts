import { requestJson } from '../../api/client'
import { memoryKeys } from '../api'

// A page's comment threads and suggested edits (AWD-53).

export type PageComment = {
  id: string
  authorId: string
  author: string | null
  /** human, or the kind of agent. */
  authorKind: string
  via?: string
  /** Empty when it was deleted. */
  body: string
  createdAt: string
  editedAt?: string
  deleted: boolean
}

export type PageThread = {
  id: string
  blockId?: string
  quote?: string
  /** False once the block or the quoted words are gone from the page. */
  anchorFound: boolean
  resolved: boolean
  resolvedBy?: string
  resolvedAt?: string
  comments: PageComment[]
}

export type SuggestedEdit = {
  op: 'replace' | 'insertBefore' | 'insertAfter' | 'delete' | 'append'
  blockId?: string
  markdown?: string
  /** The block's Markdown when the edit was suggested. */
  before?: string
}

export type PageSuggestion = {
  id: string
  pageId: string
  authorId: string
  author: string | null
  authorKind: string
  note: string | null
  baseVersion: number
  status: 'pending' | 'accepted' | 'rejected' | 'withdrawn'
  decidedBy: string | null
  decidedAt: string | null
  appliedVersion: number | null
  createdAt: string
  edits: SuggestedEdit[]
}

export type PageDiscussion = {
  pageId: string
  threads: PageThread[]
  suggestions: PageSuggestion[]
  canComment: boolean
  viewerId: string
}

/** Where a new thread is about: a top-level block's lasting id and the words in it. */
export type CommentAnchor = { blockId: string; quote: string }

const page = (pageId: string) => `/api/v1/pages/${encodeURIComponent(pageId)}`

export const discussionKey = (projectId: string, pageId: string) => [...memoryKeys.page(projectId, pageId), 'discussion'] as const

export function getDiscussion(pageId: string) {
  return requestJson<PageDiscussion>(`${page(pageId)}/discussion`)
}

export function addComment(pageId: string, input: { body: string; blockId?: string; quote?: string; replyTo?: string }) {
  return requestJson<{ id: string; threadId: string }>(`${page(pageId)}/comments`, { method: 'POST', json: input })
}

export function changeComment(pageId: string, commentId: string, input: { body: string } | { resolved: boolean }) {
  return requestJson<{ threadId: string }>(`${page(pageId)}/comments/${encodeURIComponent(commentId)}`, { method: 'PATCH', json: input })
}

export function deleteComment(pageId: string, commentId: string) {
  return requestJson<{ threadId: string }>(`${page(pageId)}/comments/${encodeURIComponent(commentId)}`, { method: 'DELETE' })
}

export function decideSuggestion(pageId: string, suggestionId: string, decision: 'accept' | 'reject') {
  return requestJson<{ status: string; version: number }>(`${page(pageId)}/suggestions/${encodeURIComponent(suggestionId)}/decision`, {
    method: 'POST',
    json: { decision },
  })
}

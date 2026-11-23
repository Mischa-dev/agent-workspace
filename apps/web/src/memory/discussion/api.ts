import { requestJson } from '../../api/client'
import { callOp, newRequestId } from '../../api/ops'
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
  /** Changes to the page made because this thread asked (C1.22). */
  changes?: ThreadChange[]
}

export type ThreadChange = {
  changeSetId: string
  actor: string | null
  createdAt: string
  undone: boolean
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
  /** Accepts and rejects suggestions: edits the page in its space (a share to comment doesn’t). */
  canDecide?: boolean
  viewerId: string
}

/** Where a new thread is about: a top-level block's lasting id and the words in it. */
export type CommentAnchor = { blockId: string; quote: string }

const page = (pageId: string) => `/api/v1/pages/${encodeURIComponent(pageId)}`

export const discussionKey = (projectId: string, pageId: string) => [...memoryKeys.page(projectId, pageId), 'discussion'] as const

export function getDiscussion(pageId: string) {
  return requestJson<PageDiscussion>(`${page(pageId)}/discussion`)
}

export async function addComment(pageId: string, input: { body: string; blockId?: string; quote?: string; replyTo?: string }) {
  const made = await callOp(
    'page_comment_create',
    { pageId, text: input.body, blockId: input.blockId, quote: input.quote, replyTo: input.replyTo },
    { requestId: newRequestId() },
  )
  return { id: made.commentId, threadId: made.threadId }
}

export async function changeComment(pageId: string, commentId: string, input: { body: string } | { resolved: boolean }) {
  const change = 'body' in input ? { text: input.body } : { resolved: input.resolved }
  const changed = await callOp('page_comment_update', { pageId, commentId, ...change }, { requestId: newRequestId() })
  return { threadId: changed.threadId }
}

export async function deleteComment(pageId: string, commentId: string) {
  const deleted = await callOp('page_comment_update', { pageId, commentId, delete: true }, { requestId: newRequestId() })
  return { threadId: deleted.threadId }
}

export async function decideSuggestion(pageId: string, suggestionId: string, decision: 'accept' | 'reject') {
  const decided = await callOp('page_suggestion_decide', { pageId, suggestionId, decision }, { requestId: newRequestId() })
  return { status: decided.status, version: decided.version }
}

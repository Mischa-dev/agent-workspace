import { useCallback, useState } from 'react'

import { useProjectExperience } from '../experience/layout'
import type { ConversationMessage, ReplyTarget } from './types'

/** How much of a message the box above the composer previews. */
const PREVIEW_CHARACTERS = 280

/** What the box above the composer shows for a message being replied to. */
export function replyTargetFor(message: ConversationMessage): ReplyTarget {
  const text = message.plainText.trim()
  return {
    messageId: message.id,
    authorName: message.author.displayName,
    text: text ? text.slice(0, PREVIEW_CHARACTERS) : 'A file',
  }
}

/**
 * The message a conversation's composer is replying to (AWD-70). It belongs
 * to one conversation, so moving to another never carries a reply over.
 * `projectId` is the project whose layout decides whether Reply shows
 * (`talk.replies`); DMs and groups pass null and always have it. When the
 * layout hides it, `start` is undefined and nothing is ever being replied to.
 */
export function useReplyTarget(conversationId: string, projectId: string | null) {
  const shown = useProjectExperience(projectId).shows('talk.replies')
  const [state, setState] = useState<{ conversationId: string; target: ReplyTarget | null }>({
    conversationId,
    target: null,
  })
  const target = shown && state.conversationId === conversationId ? state.target : null
  const set = useCallback((next: ReplyTarget | null) => setState({ conversationId, target: next }), [conversationId])
  const start = useCallback((message: ConversationMessage) => set(replyTargetFor(message)), [set])
  return { target, set, start: shown ? start : undefined }
}

import type { FileAttachment } from '../files/model'
import type {
  ConversationActor,
  ConversationWorkItemReference,
  MessageContentV1,
  MessageNode,
} from './types'

export type ComposerSubmission = {
  content: MessageContentV1
  idempotencyKey: string
  /** The draft as typed, for showing the message while it sends. */
  text?: string
  /** Files uploaded with it, sent by id. */
  attachments?: FileAttachment[]
}

/** The content of a message that is only files. */
export const BLANK_MESSAGE_CONTENT: MessageContentV1 = { version: 1, nodes: [{ type: 'text', text: '' }] }

export const MAX_MESSAGE_SCALAR_VALUES = 20_000
const MAX_NODES = 128
const MAX_MENTIONS = 32

export function createIdempotencyKey() {
  return globalThis.crypto.randomUUID()
}

/** What the server counts: the text with each mention as "@handle" and each reference as its key. */
export function composerScalarValues(text: string) {
  return [...text.trim()].length
}

const BEFORE = /[\s([{"'“‘*_~]/
const WORD = /[\p{L}\p{N}_.-]/u

/**
 * Turn composer text into content nodes. Only mentions and work items the person picked from
 * a list become nodes; typing "@name" by hand stays plain text.
 */
export function buildMessageContent(
  text: string,
  mentions: ConversationActor[],
  workItems: ConversationWorkItemReference[] = [],
): MessageContentV1 | null {
  const source = text.trim()
  if (!source) return null
  const tokens = [
    ...mentions.map((actor) => ({ token: `@${actor.handle}`, node: { type: 'mention', actorId: actor.id } as MessageNode })),
    ...workItems.map((item) => ({ token: item.key, node: { type: 'work_item_ref', workItemId: item.id } as MessageNode })),
  ].sort((left, right) => right.token.length - left.token.length)

  const nodes: MessageNode[] = []
  let buffer = ''
  let mentionCount = 0
  let index = 0
  while (index < source.length) {
    const previous = index === 0 ? ' ' : source[index - 1]
    const match = BEFORE.test(previous)
      ? tokens.find(({ token }) => {
          if (!source.startsWith(token, index)) return false
          const after = source[index + token.length]
          return after === undefined || !WORD.test(after) || (after === '.' && !WORD.test(source[index + token.length + 1] ?? ' '))
        })
      : undefined
    if (match && (match.node.type !== 'mention' || mentionCount < MAX_MENTIONS) && nodes.length < MAX_NODES - 2) {
      if (buffer) nodes.push({ type: 'text', text: buffer })
      buffer = ''
      nodes.push(match.node)
      if (match.node.type === 'mention') mentionCount += 1
      index += match.token.length
      continue
    }
    buffer += source[index]
    index += 1
  }
  if (buffer) nodes.push({ type: 'text', text: buffer })
  return nodes.length > 0 ? { version: 1, nodes } : null
}

/** Said wherever someone edits or reads a message's history: editing never hides old text. */
export const EDIT_HISTORY_HINT =
  'Earlier versions stay visible to everyone here. To take down a secret, remove the message.'

export type ComposerDraft = {
  text: string
  mentions: ConversationActor[]
  workItems: ConversationWorkItemReference[]
}

/**
 * Turn a sent message back into composer text, so editing it starts from what
 * it says now. Mentions become "@handle" and references their key, and the
 * lists come back too, so buildMessageContent rebuilds the same nodes. A
 * mention or reference the message no longer resolves is left out.
 */
export function draftFromContent(message: {
  content: MessageContentV1
  mentionedActors: ConversationActor[]
  referencedWorkItems: ConversationWorkItemReference[]
}): ComposerDraft {
  const actors = new Map(message.mentionedActors.map((actor) => [actor.id, actor]))
  const items = new Map(message.referencedWorkItems.map((item) => [item.id, item]))
  const mentions = new Map<string, ConversationActor>()
  const workItems = new Map<string, ConversationWorkItemReference>()
  let text = ''
  for (const node of message.content.nodes) {
    if (node.type === 'text') {
      text += node.text
    } else if (node.type === 'mention') {
      const actor = actors.get(node.actorId)
      if (!actor) continue
      mentions.set(actor.id, actor)
      text += `@${actor.handle}`
    } else {
      const item = items.get(node.workItemId)
      if (!item) continue
      workItems.set(item.id, item)
      text += item.key
    }
  }
  return { text, mentions: [...mentions.values()], workItems: [...workItems.values()] }
}

export type Trigger = { kind: 'mention' | 'work'; query: string; start: number }

/** The @ or # token the caret is in, if any. */
export function activeTrigger(text: string, caret: number, allowWork: boolean): Trigger | null {
  const before = text.slice(0, caret)
  const match = /(^|[\s([{"'“‘])([@#])([\p{L}\p{N}_.-]{0,40})$/u.exec(before)
  if (!match) return null
  const kind = match[2] === '@' ? 'mention' : 'work'
  if (kind === 'work' && !allowWork) return null
  return { kind, query: match[3], start: before.length - match[3].length - 1 }
}

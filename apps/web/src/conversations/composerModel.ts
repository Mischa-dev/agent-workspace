import type {
  ConversationActor,
  ConversationWorkItemReference,
  MessageContentV1,
  MessageNode,
} from './types'

export type ComposerSubmission = {
  content: MessageContentV1
  idempotencyKey: string
}

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

const BEFORE = /[\s([{"'“‘]/
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

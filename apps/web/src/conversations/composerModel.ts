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

export function createIdempotencyKey() {
  return globalThis.crypto.randomUUID()
}

export function composerScalarValues(
  text: string,
  mentions: ConversationActor[],
  workItems: ConversationWorkItemReference[] = [],
) {
  let total = [...text.trim()].length
  const uniqueActorIds = new Set<string>()
  for (const actor of mentions) {
    if (uniqueActorIds.has(actor.id)) continue
    uniqueActorIds.add(actor.id)
    if (total > 0) total += 1
    total += [...`@${actor.handle}`].length
  }
  const uniqueWorkItemIds = new Set<string>()
  for (const item of workItems) {
    if (uniqueWorkItemIds.has(item.id)) continue
    uniqueWorkItemIds.add(item.id)
    if (total > 0) total += 1
    total += [...item.key].length
  }
  return total
}

export function buildMessageContent(
  text: string,
  mentions: ConversationActor[],
  workItems: ConversationWorkItemReference[] = [],
): MessageContentV1 | null {
  const normalizedText = text.trim()
  const nodes: MessageNode[] = []
  if (normalizedText) nodes.push({ type: 'text', text: normalizedText })

  const uniqueActorIds = new Set<string>()
  for (const actor of mentions) {
    if (uniqueActorIds.has(actor.id)) continue
    uniqueActorIds.add(actor.id)
    if (nodes.length > 0) nodes.push({ type: 'text', text: ' ' })
    nodes.push({ type: 'mention', actorId: actor.id })
  }

  const uniqueWorkItemIds = new Set<string>()
  for (const item of workItems) {
    if (uniqueWorkItemIds.has(item.id)) continue
    uniqueWorkItemIds.add(item.id)
    if (nodes.length > 0) nodes.push({ type: 'text', text: ' ' })
    nodes.push({ type: 'work_item_ref', workItemId: item.id })
  }

  return nodes.length > 0 ? { version: 1, nodes } : null
}

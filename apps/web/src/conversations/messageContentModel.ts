import type { MessageContentV1, MessageNode } from './types'

const MAX_MESSAGE_NODES = 128
const MAX_MESSAGE_TEXT_CHARACTERS = 20_000
const CONTROL_CHARACTER = /\p{Cc}/u
const UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

function parseNode(value: unknown): MessageNode | null {
  const node = asRecord(value)
  if (!node || typeof node.type !== 'string') return null

  if (node.type === 'text') {
    if (Object.keys(node).some((key) => key !== 'type' && key !== 'text')) {
      return null
    }
    return typeof node.text === 'string'
      ? { type: 'text', text: node.text }
      : null
  }

  if (node.type === 'mention') {
    if (
      Object.keys(node).some((key) => key !== 'type' && key !== 'actorId') ||
      typeof node.actorId !== 'string' ||
      !UUID_PATTERN.test(node.actorId)
    ) {
      return null
    }
    return { type: 'mention', actorId: node.actorId }
  }

  if (node.type === 'work_item_ref') {
    const workItemId = node.workItemId ?? node.work_item_id
    if (
      Object.keys(node).some(
        (key) =>
          key !== 'type' && key !== 'workItemId' && key !== 'work_item_id',
      ) ||
      typeof workItemId !== 'string' ||
      !UUID_PATTERN.test(workItemId)
    ) {
      return null
    }
    return { type: 'work_item_ref', workItemId }
  }

  return null
}

export function messageTextScalarValues(value: string) {
  return [...value].length
}

export function isSafeMessageText(value: string) {
  return (
    messageTextScalarValues(value) <= MAX_MESSAGE_TEXT_CHARACTERS &&
    ![...value].some(
      (character) => {
        const codePoint = character.codePointAt(0)
        const unpairedSurrogate =
          codePoint !== undefined && codePoint >= 0xd800 && codePoint <= 0xdfff
        return (
          unpairedSurrogate ||
          (CONTROL_CHARACTER.test(character) &&
            character !== '\n' &&
            character !== '\r' &&
            character !== '\t')
        )
      },
    )
  )
}

export function parseMessageContent(value: unknown): MessageContentV1 | null {
  const content = asRecord(value)
  if (
    !content ||
    content.version !== 1 ||
    !Array.isArray(content.nodes) ||
    content.nodes.length === 0 ||
    content.nodes.length > MAX_MESSAGE_NODES ||
    Object.keys(content).some(
      (key) => key !== 'version' && key !== 'nodes',
    )
  ) {
    return null
  }

  const nodes: MessageNode[] = []
  let textCharacters = 0
  let mentions = 0
  let workItemReferences = 0
  for (const rawNode of content.nodes) {
    const node = parseNode(rawNode)
    if (!node) return null
    if (node.type === 'text') {
      if (!isSafeMessageText(node.text)) return null
      textCharacters += messageTextScalarValues(node.text)
    } else if (node.type === 'mention') {
      mentions += 1
    } else {
      workItemReferences += 1
    }
    if (
      textCharacters > MAX_MESSAGE_TEXT_CHARACTERS ||
      mentions > 32 ||
      workItemReferences > 32
    ) {
      return null
    }
    nodes.push(node)
  }

  return { version: 1, nodes }
}

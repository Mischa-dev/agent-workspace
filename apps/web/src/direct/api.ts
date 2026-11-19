import { useQuery } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { callOp } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { normalizeConversationActor } from '../conversations/sidebarApi'
import type { ConversationActor } from '../conversations/types'

export type DirectConversation = {
  id: string
  kind: 'dm' | 'group'
  name: string
  version: number
  members: ConversationActor[]
  highestSequence: number
  unread: boolean
  /** Others' unread messages, at most 99: a DM row shows the number (C2.3). */
  unreadMessages: number
  mentions: number
  /** Set to tell you nothing: still unread, but never bold or counted. */
  muted: boolean
  lastActivityAt: string
}

export type DirectOverview = {
  projectId: string
  /** The workspace the Messages space belongs to. */
  organizationId: string
  conversations: DirectConversation[]
  people: ConversationActor[]
  capabilities: { startConversation: boolean; manageAgents: boolean }
}

export const directQueryKey = queryKeys.direct

type WireRecord = Record<string, unknown>

function asRecord(value: unknown): WireRecord {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as WireRecord)
    : {}
}

function actors(value: unknown): ConversationActor[] {
  return Array.isArray(value)
    ? value
        .map(normalizeConversationActor)
        .filter((actor): actor is ConversationActor => actor !== null)
    : []
}

function normalizeConversation(value: unknown): DirectConversation | null {
  const record = asRecord(value)
  if (
    typeof record.id !== 'string' ||
    (record.kind !== 'dm' && record.kind !== 'group')
  ) {
    return null
  }
  return {
    id: record.id,
    kind: record.kind,
    name: typeof record.name === 'string' ? record.name : '',
    version: typeof record.version === 'number' ? record.version : 1,
    members: actors(record.members),
    highestSequence:
      typeof record.highestSequence === 'number' ? record.highestSequence : 0,
    unread: record.unread === true,
    unreadMessages: typeof record.unreadMessages === 'number' ? Math.max(0, record.unreadMessages) : 0,
    mentions: typeof record.mentions === 'number' ? Math.max(0, record.mentions) : 0,
    muted: record.muted === true,
    lastActivityAt:
      typeof record.lastActivityAt === 'string' ? record.lastActivityAt : '',
  }
}

export async function getDirectOverview(): Promise<DirectOverview> {
  const root = asRecord(await requestJson<unknown>('/api/v1/direct'))
  const capabilities = asRecord(root.capabilities)
  if (typeof root.projectId !== 'string') {
    throw new Error('Direct messages are unavailable.')
  }
  return {
    projectId: root.projectId,
    organizationId: typeof root.organizationId === 'string' ? root.organizationId : '',
    conversations: Array.isArray(root.conversations)
      ? root.conversations
          .map(normalizeConversation)
          .filter((item): item is DirectConversation => item !== null)
      : [],
    people: actors(root.people),
    capabilities: {
      startConversation: capabilities.startConversation === true,
      manageAgents: capabilities.manageAgents === true,
    },
  }
}

function conversationId(value: unknown) {
  const id = asRecord(asRecord(value).conversation).id
  if (typeof id !== 'string') throw new Error('Conversation was not created.')
  return id
}

export async function openDm(actorId: string) {
  return conversationId(
    await requestJson<unknown>('/api/v1/direct/dms', {
      method: 'POST',
      json: { actorId },
    }),
  )
}

export async function createGroup(input: {
  name?: string
  memberIds: string[]
  idempotencyKey: string
}) {
  return conversationId(
    await requestJson<unknown>('/api/v1/direct/groups', {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      json: {
        name: input.name?.trim() ? input.name.trim() : null,
        memberIds: input.memberIds,
      },
    }),
  )
}

export async function addGroupMember(conversationId: string, actorId: string) {
  await callOp('conversation_members_update', { conversationId, add: [actorId] })
}

export async function removeGroupMember(conversationId: string, actorId: string) {
  await callOp('conversation_members_update', { conversationId, remove: [actorId] })
}

export function useDirectOverview() {
  return useQuery({
    queryKey: directQueryKey,
    queryFn: getDirectOverview,
    staleTime: 15_000,
    retry: false,
  })
}

/** DMs are named after the other person; groups keep their own name. */
export function conversationTitle(
  conversation: Pick<DirectConversation, 'kind' | 'name' | 'members'>,
  viewerId: string,
) {
  if (conversation.kind === 'group') return conversation.name
  const other = conversation.members.find((member) => member.id !== viewerId)
  // A DM with only yourself in it is your note to self (C2.32).
  if (!other && conversation.members.length > 0) return 'Note to self'
  return other?.displayName ?? conversation.name
}

import { useQuery } from '@tanstack/react-query'

import { requestJson } from '../api/client'
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

export const directQueryKey = ['direct'] as const

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
  await requestJson<unknown>(
    `/api/v1/conversations/${encodeURIComponent(conversationId)}/members`,
    { method: 'POST', json: { actorId } },
  )
}

export async function removeGroupMember(
  conversationId: string,
  actorId: string,
) {
  await requestJson<unknown>(
    `/api/v1/conversations/${encodeURIComponent(conversationId)}/members/${encodeURIComponent(actorId)}/remove`,
    { method: 'POST' },
  )
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
  return other?.displayName ?? conversation.name
}
